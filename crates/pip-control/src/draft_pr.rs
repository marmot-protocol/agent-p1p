//! Controller-owned draft pull-request publication for a recorded builder head.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use pip_contracts::{BuilderOutcome, BuilderResult, PlannerOutcome, WorkerResult};
use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_executor::{
    GitPublicationSpec, GitPublisher, ProcessGitRunner, PublicationError, PublicationResult,
    SignedCommit,
};
use pip_github::{GitHubError, GitHubWriter, MutationResult, MutationTransport, PullRequestSpec};
use pip_store::{EvidenceInput, ImmutableCaseHistory, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const PUBLISH_EFFECT: &str = "PUBLISH_DRAFT_PULL_REQUEST";

pub trait DraftPullRequestWriter {
    fn ensure_draft_pull_request(
        &self,
        spec: &PullRequestSpec,
    ) -> Result<MutationResult, GitHubError>;
}

impl<T: MutationTransport> DraftPullRequestWriter for GitHubWriter<T> {
    fn ensure_draft_pull_request(
        &self,
        spec: &PullRequestSpec,
    ) -> Result<MutationResult, GitHubError> {
        GitHubWriter::ensure_draft_pull_request(self, spec)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchPublicationRequest {
    pub worktree_root: PathBuf,
    pub worktree: PathBuf,
    pub remote: String,
    pub expected_remote_url: String,
    pub branch: String,
    pub local_head: String,
    pub parent_head: String,
    pub expected_remote_head: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchPublication {
    pub result: PublicationResult,
    pub signed: Option<SignedCommit>,
}

#[derive(Clone, Copy)]
pub struct CommitSigningCredentials<'a> {
    pub identity_file: &'a Path,
    pub key_file: &'a Path,
}

pub trait BranchPublisher {
    fn publish_branch(
        &self,
        request: &BranchPublicationRequest,
    ) -> Result<BranchPublication, PublicationError>;
}

impl BranchPublicationRequest {
    fn spec(&self) -> Result<GitPublicationSpec, PublicationError> {
        let request = self;
        let local_head =
            GitSha::from_str(&request.local_head).map_err(|_| PublicationError::InvalidSpec)?;
        let expected_remote_head = request
            .expected_remote_head
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| PublicationError::InvalidSpec)?;
        GitPublicationSpec::new_scoped(
            &request.worktree_root,
            &request.worktree,
            &request.remote,
            &request.expected_remote_url,
            &request.branch,
            local_head,
            expected_remote_head,
        )
    }
}

struct ControllerPublisher<'a> {
    signing: Option<CommitSigningCredentials<'a>>,
    actor: u64,
    git_askpass: &'a Path,
    github_token_file: &'a Path,
}

impl BranchPublisher for ControllerPublisher<'_> {
    fn publish_branch(
        &self,
        request: &BranchPublicationRequest,
    ) -> Result<BranchPublication, PublicationError> {
        // Resolve this capability only after its durable effect is claimed.
        // Missing credentials must never turn idle collection into an outage.
        let signing = self.signing.ok_or_else(|| {
            PublicationError::Process("commit signing credentials are not configured".into())
        })?;
        let identity = crate::commit_signing::load_identity(signing.identity_file, self.actor)?;
        let publisher = GitPublisher::new(
            ProcessGitRunner,
            "git",
            Duration::from_secs(30),
            1024 * 1024,
        )?
        .with_askpass(self.git_askpass, self.github_token_file)?;
        let (result, signed) = publisher.publish_signed(
            &request.spec()?,
            request
                .parent_head
                .parse()
                .map_err(|_| PublicationError::InvalidSpec)?,
            &identity,
            signing.key_file,
        )?;
        Ok(BranchPublication {
            result,
            signed: Some(signed),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum DraftPullRequestCycle {
    Idle,
    AuthorizationBlocked,
    Published {
        case_key: String,
        pull_request_number: u64,
        head_sha: String,
    },
}

#[derive(Debug)]
pub enum DraftPullRequestError {
    GitHub(GitHubError),
    Store(StoreError),
    Controller(ControllerError),
    Publication(PublicationError),
    MissingAutomationActor,
    InvalidCase,
    InvalidBuildJoin,
    PullRequestIdentityDrift,
    UnexpectedMutationResult,
    Serialization(String),
}

impl fmt::Display for DraftPullRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::Publication(error) => error.fmt(formatter),
            Self::MissingAutomationActor => {
                formatter.write_str("draft PR publication requires an automation actor")
            }
            Self::InvalidCase => formatter.write_str("draft PR publication case is invalid"),
            Self::InvalidBuildJoin => formatter
                .write_str("draft PR publication requires the builder result bound to its accepted build event"),
            Self::PullRequestIdentityDrift => {
                formatter.write_str("draft PR publication changed the case pull request")
            }
            Self::UnexpectedMutationResult => {
                formatter.write_str("draft PR mutation returned an unexpected result")
            }
            Self::Serialization(error) => {
                write!(formatter, "draft PR serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for DraftPullRequestError {}

macro_rules! error_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for DraftPullRequestError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(GitHubError, GitHub);
error_from!(StoreError, Store);
error_from!(ControllerError, Controller);
error_from!(PublicationError, Publication);

#[allow(clippy::too_many_arguments)]
pub fn publish_draft_pull_request_once<'a, W: DraftPullRequestWriter>(
    writer: &W,
    scope: impl Into<crate::RepositoryScope<'a>>,
    store: &mut Store,
    git_askpass: &Path,
    github_token_file: &Path,
    signing: Option<CommitSigningCredentials<'_>>,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<DraftPullRequestCycle, DraftPullRequestError> {
    let scope = scope.into();
    let policy = scope.policy;
    let publisher = ControllerPublisher {
        signing,
        actor: policy.github.automation_actor_id.unwrap_or(0),
        git_askpass,
        github_token_file,
    };
    publish_draft_pull_request_once_with(
        writer,
        &publisher,
        scope,
        store,
        now,
        owner,
        lease_seconds,
        authorization_valid,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn publish_draft_pull_request_once_with<'a, W: DraftPullRequestWriter, P: BranchPublisher>(
    writer: &W,
    publisher: &P,
    scope: impl Into<crate::RepositoryScope<'a>>,
    store: &mut Store,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<DraftPullRequestCycle, DraftPullRequestError> {
    let scope = scope.into();
    let policy = scope.policy;
    if !authorization_valid {
        return Ok(DraftPullRequestCycle::AuthorizationBlocked);
    }
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(DraftPullRequestError::MissingAutomationActor)?;
    let Some(claimed) = scope.claim(store, owner, now, lease_seconds, &[PUBLISH_EFFECT])? else {
        return Ok(DraftPullRequestCycle::Idle);
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or(DraftPullRequestError::InvalidCase)?;
    if !matches!(case.state.as_str(), "BUILDING" | "REMEDIATING")
        || case.repository_id != policy.repository.id
        || case.policy_revision != policy.revision
        || case.state_revision != claimed.state_revision
        || case.plan_version == 0
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(DraftPullRequestError::InvalidCase);
    }
    let (build, parent, requires_signing) = match active_build(store, &case) {
        Ok(build) => build,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error);
        }
    };
    let head_sha = build
        .head_sha
        .as_deref()
        .ok_or(DraftPullRequestError::InvalidBuildJoin)?;
    let branch = format!(
        "{}repo-{}/issue-{}/workflow-{}",
        policy.branch_prefix, case.repository_id, case.issue_number, case.workflow_version
    );
    let worktree_root = PathBuf::from(&policy.workspace);
    let worktree = worktree_root.join(format!(
        "repo-{}-issue-{}-workflow-{}",
        case.repository_id, case.issue_number, case.workflow_version
    ));
    let publication = publisher.publish_branch(&BranchPublicationRequest {
        worktree_root,
        worktree,
        remote: "origin".into(),
        expected_remote_url: format!(
            "https://github.com/{}/{}.git",
            policy.repository.owner, policy.repository.name
        ),
        branch: branch.clone(),
        local_head: head_sha.into(),
        parent_head: parent.to_string(),
        expected_remote_head: case.head_sha.clone(),
    });
    let publication = match publication {
        Ok(publication) => publication,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    if (requires_signing && publication.signed.is_none())
        || publication.signed.as_ref().is_some_and(|signed| {
            signed.source_head.to_string() != head_sha
                || signed.parent != parent
                || signed.head == signed.source_head
                || !signed.signer_fingerprint.starts_with("SHA256:")
                || signed.signer_fingerprint.len() <= 7
        })
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(DraftPullRequestError::InvalidBuildJoin);
    }
    let head_sha = publication
        .signed
        .as_ref()
        .map(|signed| signed.head.to_string())
        .unwrap_or_else(|| head_sha.into());
    let branch_publication = match publication.result {
        PublicationResult::Created => "created",
        PublicationResult::Updated => "updated",
        PublicationResult::Existing => "existing",
    };
    let signing = publication.signed.as_ref().map(|signed| {
        json!({
            "source_head": signed.source_head.to_string(), "head": signed.head.to_string(),
            "tree": signed.tree.to_string(), "parent": signed.parent.to_string(),
            "signer_fingerprint": signed.signer_fingerprint,
        })
    });
    let body = render_body(&case, &build, &head_sha);
    let result = writer.ensure_draft_pull_request(&PullRequestSpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        repository_id: policy.repository.id,
        effect_id: format!("{}:draft-pr", case.case_key),
        expected_actor_id: expected_actor,
        title: format!(
            "Pip: issue #{} (workflow {})",
            case.issue_number, case.workflow_version
        ),
        body,
        head_branch: branch.clone(),
        head_sha: head_sha.clone(),
        base_branch: policy.repository.default_branch.clone(),
    });
    let (mutation, pr_number) = match result {
        Ok(MutationResult::Created(number)) => ("created", number),
        Ok(MutationResult::Existing(number)) => ("existing", number),
        Ok(MutationResult::Updated(number)) => ("updated", number),
        Ok(MutationResult::Merged(_)) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(DraftPullRequestError::UnexpectedMutationResult);
        }
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    if case.pr_number.is_some_and(|current| current != pr_number) {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(DraftPullRequestError::PullRequestIdentityDrift);
    }
    let evidence = EvidenceInput {
        evidence_id: format!("evidence-draft-pr-{}", claimed.effect_id),
        kind: "GITHUB_DRAFT_PULL_REQUEST_PUBLICATION".into(),
        source: format!("github-pr-{pr_number}"),
        payload: json!({
            "actor_id": expected_actor,
            "branch": branch,
            "branch_publication": branch_publication,
            "head_sha": head_sha,
            "mutation": mutation,
            "pull_request_number": pr_number,
            "task_id": build.common.task_id,
            "signing": signing,
        }),
    };
    let command = match published_command(&case, &build, &head_sha, pr_number, now, evidence) {
        Ok(command) => command,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error);
        }
    };
    if let Err(error) = LedgerController::apply(store, &policy.case_policy(), &command) {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(error.into());
    }
    Ok(DraftPullRequestCycle::Published {
        case_key: case.case_key,
        pull_request_number: pr_number,
        head_sha,
    })
}

fn active_build(
    store: &Store,
    case: &StoredCase,
) -> Result<(BuilderResult, GitSha, bool), DraftPullRequestError> {
    // The pending effect and this revision were committed with one accepted
    // build. That event/run identity is authoritative, not a worker's counter
    // or an ambiguous scan of all earlier builds in the same plan.
    let history = store.immutable_history_for_case(&case.case_key)?;
    if history
        .events
        .last()
        .is_some_and(|event| event.event_type == "PUBLICATION_RETRY_AUTHORIZED")
    {
        return crate::publication_retry::authorized_build(&history, case)
            .map(|(build, parent)| (build, parent, true))
            .map_err(|_| DraftPullRequestError::InvalidBuildJoin);
    }
    let event = history
        .events
        .last()
        .filter(|event| {
            event.state_revision == case.state_revision && event.event_type == "BUILD_RECORDED"
        })
        .ok_or(DraftPullRequestError::InvalidBuildJoin)?;
    let run = history
        .runs
        .iter()
        .find(|run| run.event_id == event.event_id)
        .filter(|run| run.role == "builder" && run.payload_sha256 == event.payload_sha256)
        .ok_or(DraftPullRequestError::InvalidBuildJoin)?;
    let result: WorkerResult = serde_json::from_value(run.payload.clone())
        .map_err(|error| DraftPullRequestError::Serialization(error.to_string()))?;
    match result {
        WorkerResult::Builder(build)
            if build.outcome == BuilderOutcome::ReviewReady
                && build.plan_version == case.plan_version
                && build.common.task_id == run.task_id =>
        {
            let parent = if let Some(head) = &case.head_sha {
                head.parse()
                    .map_err(|_| DraftPullRequestError::InvalidBuildJoin)?
            } else {
                planned_base(&history, case)?
            };
            Ok((build, parent, false))
        }
        _ => Err(DraftPullRequestError::InvalidBuildJoin),
    }
}

/// Join a controller publication to the exact accepted source result. Signing
/// changes commit identity, never the worker's result or the reviewed PR head.
pub(crate) fn published_builder(
    history: &ImmutableCaseHistory,
    case: &StoredCase,
) -> Result<Option<BuilderResult>, DraftPullRequestError> {
    let Some(event) = history
        .events
        .iter()
        .rev()
        .find(|event| event.event_type == "REVIEW_READY")
    else {
        return Ok(None);
    };
    let publication = &event.payload["publication"];
    let signing = &publication["signing"];
    if signing.is_null() {
        return Ok(None); // Historical unsigned publications retain their exact-head join.
    }
    publication_source(history, case, true).map(|(build, _)| Some(build))
}

/// Both initial signing recovery and final review use the same immutable join.
pub(crate) fn publication_source(
    history: &ImmutableCaseHistory,
    case: &StoredCase,
    signed: bool,
) -> Result<(BuilderResult, String), DraftPullRequestError> {
    let invalid = || DraftPullRequestError::InvalidBuildJoin;
    let event = history
        .events
        .iter()
        .rev()
        .find(|event| event.event_type == "REVIEW_READY")
        .ok_or_else(invalid)?;
    let publication = &event.payload["publication"];
    let signing = &publication["signing"];
    let head = case.head_sha.as_deref().ok_or_else(invalid)?;
    if !payload_matches(&event.payload, &event.payload_sha256)
        || publication["head_sha"].as_str() != Some(head)
        || publication["pull_request_number"].as_u64() != case.pr_number
        || (signed
            && (signing["head"].as_str() != Some(head)
                || !signing["signer_fingerprint"]
                    .as_str()
                    .is_some_and(|value| value.starts_with("SHA256:") && value.len() > 7)))
        || (!signed && !signing.is_null())
        || !history.evidence.iter().any(|evidence| {
            evidence.kind == "GITHUB_DRAFT_PULL_REQUEST_PUBLICATION"
                && evidence.payload == *publication
                && payload_matches(&evidence.payload, &evidence.payload_sha256)
        })
    {
        return Err(invalid());
    }
    for field in ["source_head", "head", "tree", "parent"]
        .into_iter()
        .filter(|_| signed)
    {
        signing[field]
            .as_str()
            .ok_or_else(invalid)?
            .parse::<GitSha>()
            .map_err(|_| invalid())?;
    }
    let accepted = history
        .events
        .iter()
        .rev()
        .find(|candidate| {
            candidate.state_revision < event.state_revision
                && candidate.event_type == "BUILD_RECORDED"
        })
        .ok_or_else(invalid)?;
    let run = history
        .runs
        .iter()
        .find(|run| run.event_id == accepted.event_id)
        .ok_or_else(invalid)?;
    if run.case_key != case.case_key
        || run.role != "builder"
        || publication["task_id"].as_str() != Some(run.task_id.as_str())
        || run.payload != event.payload["builder_result"]
        || run.payload != accepted.payload
        || !payload_matches(&run.payload, &run.payload_sha256)
        || !payload_matches(&accepted.payload, &accepted.payload_sha256)
    {
        return Err(invalid());
    }
    let WorkerResult::Builder(build) =
        serde_json::from_value(run.payload.clone()).map_err(|_| invalid())?
    else {
        return Err(invalid());
    };
    if build.common.task_id != run.task_id
        || build.plan_version != case.plan_version
        || build.outcome != BuilderOutcome::ReviewReady
        || (signed
            && (build.head_sha.as_deref() != signing["source_head"].as_str()
                || build.head_sha.as_deref() == Some(head)))
        || (!signed && build.head_sha.as_deref() != Some(head))
    {
        return Err(invalid());
    }
    Ok((build, accepted.event_id.clone()))
}

pub(crate) fn planned_base(
    history: &ImmutableCaseHistory,
    case: &StoredCase,
) -> Result<GitSha, DraftPullRequestError> {
    let mut planned = Vec::new();
    for run in history.runs.iter().filter(|run| run.role == "planner") {
        let result: WorkerResult = serde_json::from_value(run.payload.clone())
            .map_err(|error| DraftPullRequestError::Serialization(error.to_string()))?;
        if let WorkerResult::Planner(plan) = result
            && plan.outcome == PlannerOutcome::Proceed
            && plan.plan_version == case.plan_version
            && plan.common.task_id == run.task_id
            && payload_matches(&run.payload, &run.payload_sha256)
        {
            planned.push(plan.planned_base_sha);
        }
    }
    let [head] = planned.as_slice() else {
        return Err(DraftPullRequestError::InvalidBuildJoin);
    };
    head.parse()
        .map_err(|_| DraftPullRequestError::InvalidBuildJoin)
}

pub(crate) fn payload_matches(payload: &serde_json::Value, expected: &str) -> bool {
    serde_json::to_vec(payload).is_ok_and(|bytes| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            == expected
    })
}

fn render_body(case: &StoredCase, build: &BuilderResult, head_sha: &str) -> String {
    use crate::publication_text::{bullets, prose};
    let checks = bullets(&build.local_checks, "No local checks reported.");
    let resolutions = if build.finding_resolutions.is_empty() {
        "No findings required remediation.".into()
    } else {
        build
            .finding_resolutions
            .iter()
            .map(|resolution| {
                format!(
                    "- **{}:** {}\n\n  Checks: {}",
                    prose(&resolution.finding_id),
                    prose(&resolution.resolution_summary),
                    prose(&resolution.tests.join("; ")),
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    format!(
        "## Pip implementation for #{}\n\nPlan version: {} · Remediation round: {}\n\nCommit: `{}`\n\n### Local checks\n\n{checks}\n\n### Findings addressed\n\n{resolutions}\n\nChecks are builder-reported; required CI and independent reviews are evaluated separately. Full structured build evidence is retained by Pip.",
        case.issue_number, build.plan_version, case.remediation_round, head_sha,
    )
}

fn published_command(
    case: &StoredCase,
    build: &BuilderResult,
    head_sha: &str,
    pr_number: u64,
    now: u64,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, DraftPullRequestError> {
    Ok(WorkflowCommand {
        case_id: CaseId::new(
            RepositoryId::new(
                NonZeroU64::new(case.repository_id).ok_or(DraftPullRequestError::InvalidCase)?,
            ),
            IssueNumber::new(
                NonZeroU64::new(case.issue_number).ok_or(DraftPullRequestError::InvalidCase)?,
            ),
            WorkflowVersion::new(
                NonZeroU32::new(case.workflow_version).ok_or(DraftPullRequestError::InvalidCase)?,
            ),
        ),
        event_id: EventId::from_str(&format!(
            "event-draft-pr-published-repo{}-issue{}-workflow{}-revision{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| DraftPullRequestError::InvalidCase)?,
        observed_at: ObservedAt::new(now),
        expected_state: CaseState::from_str(&case.state)
            .map_err(|_| DraftPullRequestError::InvalidCase)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(DraftPullRequestError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(DraftPullRequestError::InvalidCase)?,
        ),
        remediation_round: case.remediation_round,
        plan_version: NonZeroU32::new(case.plan_version).map(PlanVersion::new),
        pr_number: case
            .pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        head_sha: case
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| DraftPullRequestError::InvalidCase)?,
        event: Event::ReviewReady,
        accepted_plan_version: None,
        next_pr_number: NonZeroU64::new(pr_number).map(PullRequestNumber::new),
        next_head_sha: Some(
            GitSha::from_str(head_sha).map_err(|_| DraftPullRequestError::InvalidBuildJoin)?,
        ),
        event_payload: json!({
            "builder_result": build,
            "publication": evidence.payload,
        }),
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}
