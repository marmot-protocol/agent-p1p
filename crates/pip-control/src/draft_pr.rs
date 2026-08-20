//! Controller-owned draft pull-request publication for a recorded builder head.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use pip_contracts::{BuilderOutcome, BuilderResult, WorkerResult};
use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_executor::{
    GitPublicationSpec, GitPublisher, GitRunner, ProcessGitRunner, PublicationError,
    PublicationResult,
};
use pip_github::{GitHubError, GitHubWriter, MutationResult, MutationTransport, PullRequestSpec};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;

use crate::RepositoryPolicy;

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
    pub expected_remote_head: Option<String>,
}

pub trait BranchPublisher {
    fn publish_branch(
        &self,
        request: &BranchPublicationRequest,
    ) -> Result<PublicationResult, PublicationError>;
}

impl<R: GitRunner> BranchPublisher for GitPublisher<R> {
    fn publish_branch(
        &self,
        request: &BranchPublicationRequest,
    ) -> Result<PublicationResult, PublicationError> {
        let local_head =
            GitSha::from_str(&request.local_head).map_err(|_| PublicationError::InvalidSpec)?;
        let expected_remote_head = request
            .expected_remote_head
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| PublicationError::InvalidSpec)?;
        let spec = GitPublicationSpec::new_scoped(
            &request.worktree_root,
            &request.worktree,
            &request.remote,
            &request.expected_remote_url,
            &request.branch,
            local_head,
            expected_remote_head,
        )?;
        self.publish(&spec)
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
                .write_str("draft PR publication requires one exact active-round builder result"),
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
pub fn publish_draft_pull_request_once<W: DraftPullRequestWriter>(
    writer: &W,
    policy: &RepositoryPolicy,
    store: &mut Store,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<DraftPullRequestCycle, DraftPullRequestError> {
    let publisher = GitPublisher::new(
        ProcessGitRunner,
        "git",
        Duration::from_secs(30),
        1024 * 1024,
    )?;
    publish_draft_pull_request_once_with(
        writer,
        &publisher,
        policy,
        store,
        now,
        owner,
        lease_seconds,
        authorization_valid,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn publish_draft_pull_request_once_with<W: DraftPullRequestWriter, P: BranchPublisher>(
    writer: &W,
    publisher: &P,
    policy: &RepositoryPolicy,
    store: &mut Store,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<DraftPullRequestCycle, DraftPullRequestError> {
    if !authorization_valid {
        return Ok(DraftPullRequestCycle::AuthorizationBlocked);
    }
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(DraftPullRequestError::MissingAutomationActor)?;
    let Some(claimed) =
        store.claim_effect_matching(owner, now, lease_seconds, &[PUBLISH_EFFECT])?
    else {
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
    let build = match active_build(store, &case) {
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
        expected_remote_head: case.head_sha.clone(),
    });
    let branch_publication = match publication {
        Ok(PublicationResult::Created) => "created",
        Ok(PublicationResult::Updated) => "updated",
        Ok(PublicationResult::Existing) => "existing",
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    let body = render_body(&case, &build)?;
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
        head_sha: head_sha.into(),
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
        }),
    };
    let command = match published_command(&case, &build, pr_number, now, evidence) {
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
        head_sha: head_sha.into(),
    })
}

fn active_build(store: &Store, case: &StoredCase) -> Result<BuilderResult, DraftPullRequestError> {
    let expected_round = case.remediation_round.saturating_add(1);
    let mut matching = Vec::new();
    for run in store.runs_for_case(&case.case_key)? {
        let result: WorkerResult = serde_json::from_value(run.payload)
            .map_err(|error| DraftPullRequestError::Serialization(error.to_string()))?;
        if let WorkerResult::Builder(build) = result
            && build.outcome == BuilderOutcome::ReviewReady
            && build.plan_version == case.plan_version
            && build.build_round == expected_round
        {
            matching.push(build);
        }
    }
    match matching.as_slice() {
        [build] => Ok(build.clone()),
        _ => Err(DraftPullRequestError::InvalidBuildJoin),
    }
}

fn render_body(case: &StoredCase, build: &BuilderResult) -> Result<String, DraftPullRequestError> {
    let checks = serde_json::to_string_pretty(&build.local_checks)
        .map_err(|error| DraftPullRequestError::Serialization(error.to_string()))?;
    let resolutions = serde_json::to_string_pretty(&build.finding_resolutions)
        .map_err(|error| DraftPullRequestError::Serialization(error.to_string()))?;
    Ok(format!(
        "## Pip controller-owned draft PR\n\nCase: `{}`\n\nPlan version: {}\n\nBuild round: {}\n\nExact head: `{}`\n\n### Local checks\n\n```json\n{checks}\n```\n\n### Finding resolutions\n\n```json\n{resolutions}\n```\n\nPip builder task: `{}`",
        case.case_key,
        build.plan_version,
        build.build_round,
        build
            .head_sha
            .as_deref()
            .ok_or(DraftPullRequestError::InvalidBuildJoin)?,
        build.common.task_id,
    ))
}

fn published_command(
    case: &StoredCase,
    build: &BuilderResult,
    pr_number: u64,
    now: u64,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, DraftPullRequestError> {
    let head_sha = build
        .head_sha
        .as_deref()
        .ok_or(DraftPullRequestError::InvalidBuildJoin)?;
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
