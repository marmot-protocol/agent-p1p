//! Guarded, restart-safe merge preparation and exact-head execution.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{FinalOutcome, WorkerResult};
use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_github::{
    GitHubError, GitHubReader, GitHubWriter, MergeModePolicy, MergeSpec, MutationResult,
    MutationTransport, PullRequestEvidence, PullRequestReadySpec, ReadTransport,
    ReviewThreadSnapshot,
};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;

use crate::final_preflight::final_gate_blockers;
use crate::{FinalPreflightError, RepositoryPolicy};

const MERGE_EFFECTS: [&str; 2] = ["BEGIN_MERGE", "EXECUTE_MERGE"];

pub trait MergeSource {
    fn pull_request(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError>;

    fn review_threads(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<Vec<ReviewThreadSnapshot>, GitHubError>;
}

impl<T: ReadTransport> MergeSource for GitHubReader<T> {
    fn pull_request(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError> {
        self.read_pull_request(owner, repository, repository_id, pull_request_number)
    }

    fn review_threads(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<Vec<ReviewThreadSnapshot>, GitHubError> {
        self.read_review_threads(owner, repository, repository_id, pull_request_number)
    }
}

pub trait MergeWriter {
    fn mark_ready(&self, spec: &PullRequestReadySpec) -> Result<MutationResult, GitHubError>;
    fn merge(
        &self,
        spec: &MergeSpec,
        policy: MergeModePolicy,
    ) -> Result<MutationResult, GitHubError>;
}

impl<T: MutationTransport> MergeWriter for GitHubWriter<T> {
    fn mark_ready(&self, spec: &PullRequestReadySpec) -> Result<MutationResult, GitHubError> {
        self.mark_pull_request_ready(spec)
    }

    fn merge(
        &self,
        spec: &MergeSpec,
        policy: MergeModePolicy,
    ) -> Result<MutationResult, GitHubError> {
        self.merge_pull_request(spec, policy)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum MergeCycle {
    Disabled,
    Idle,
    AuthorizationBlocked,
    Pending {
        case_key: String,
        blockers: Vec<String>,
    },
    Prepared {
        case_key: String,
        head_sha: String,
    },
    Merged {
        case_key: String,
        merge_commit_sha: String,
    },
}

#[derive(Debug)]
pub enum MergeCycleError {
    GitHub(GitHubError),
    Store(StoreError),
    Controller(ControllerError),
    FinalPreflight(FinalPreflightError),
    InvalidCase,
    MissingAutomationActor,
    MissingFinalReady,
    UnexpectedMutationResult,
}

impl fmt::Display for MergeCycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::FinalPreflight(error) => error.fmt(formatter),
            Self::InvalidCase => formatter.write_str("guarded merge case is invalid"),
            Self::MissingAutomationActor => {
                formatter.write_str("guarded merge requires an automation actor")
            }
            Self::MissingFinalReady => {
                formatter.write_str("guarded merge requires an exact-head final READY result")
            }
            Self::UnexpectedMutationResult => {
                formatter.write_str("guarded merge mutation returned an unexpected result")
            }
        }
    }
}

impl std::error::Error for MergeCycleError {}

macro_rules! error_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for MergeCycleError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(GitHubError, GitHub);
error_from!(StoreError, Store);
error_from!(ControllerError, Controller);
error_from!(FinalPreflightError, FinalPreflight);

#[allow(clippy::too_many_arguments)]
pub fn reconcile_merge_once<S: MergeSource, W: MergeWriter>(
    source: &S,
    writer: &W,
    policy: &RepositoryPolicy,
    store: &mut Store,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<MergeCycle, MergeCycleError> {
    if policy.merge.is_shadow() || !policy.merge.autonomous {
        return Ok(MergeCycle::Disabled);
    }
    if !authorization_valid {
        return Ok(MergeCycle::AuthorizationBlocked);
    }
    let Some(claimed) = store.claim_effect_matching(owner, now, lease_seconds, &MERGE_EFFECTS)?
    else {
        return Ok(MergeCycle::Idle);
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or(MergeCycleError::InvalidCase)?;
    let valid_state = match claimed.effect_type.as_str() {
        "BEGIN_MERGE" => case.state == "READY_TO_MERGE",
        "EXECUTE_MERGE" => case.state == "MERGING",
        _ => false,
    };
    if !valid_state
        || case.state_revision != claimed.state_revision
        || case.repository_id != policy.repository.id
        || case.policy_revision != policy.revision
        || case.pr_number.is_none()
        || case.head_sha.is_none()
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(MergeCycleError::InvalidCase);
    }
    if !has_final_ready(store, &case)? {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(MergeCycleError::MissingFinalReady);
    }
    let result = match claimed.effect_type.as_str() {
        "BEGIN_MERGE" => prepare(
            source,
            writer,
            policy,
            store,
            &case,
            &claimed.effect_id,
            now,
            owner,
        ),
        "EXECUTE_MERGE" => execute(
            source,
            writer,
            policy,
            store,
            &case,
            &claimed.effect_id,
            now,
            owner,
        ),
        _ => unreachable!("effect type validated"),
    };
    if result.is_err() {
        store.release_effect(&claimed.effect_id, owner)?;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn prepare<S: MergeSource, W: MergeWriter>(
    source: &S,
    writer: &W,
    policy: &RepositoryPolicy,
    store: &mut Store,
    case: &StoredCase,
    effect_id: &str,
    now: u64,
    owner: &str,
) -> Result<MergeCycle, MergeCycleError> {
    let (evidence, threads) = read_gate(source, policy, case)?;
    let blockers = final_gate_blockers(store, case, policy, &evidence, &threads, None)?;
    if !blockers.is_empty() {
        store.release_effect(effect_id, owner)?;
        return Ok(MergeCycle::Pending {
            case_key: case.case_key.clone(),
            blockers,
        });
    }
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(MergeCycleError::MissingAutomationActor)?;
    let result = writer.mark_ready(&PullRequestReadySpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        repository_id: policy.repository.id,
        pull_request_number: case.pr_number.ok_or(MergeCycleError::InvalidCase)?,
        expected_actor_id: expected_actor,
        expected_head_branch: expected_branch(policy, case),
        expected_head_sha: case.head_sha.clone().ok_or(MergeCycleError::InvalidCase)?,
        expected_base_branch: policy.repository.default_branch.clone(),
        client_mutation_id: format!("{}:ready", case.case_key),
    });
    match result {
        Ok(MutationResult::Existing(number) | MutationResult::Updated(number))
            if Some(number) == case.pr_number => {}
        Ok(_) => return Err(MergeCycleError::UnexpectedMutationResult),
        Err(error) => return Err(error.into()),
    }
    let (after, after_threads) = read_gate(source, policy, case)?;
    let blockers = final_gate_blockers(store, case, policy, &after, &after_threads, Some(false))?;
    if !blockers.is_empty() {
        store.release_effect(effect_id, owner)?;
        return Ok(MergeCycle::Pending {
            case_key: case.case_key.clone(),
            blockers,
        });
    }
    let command = workflow(
        case,
        now,
        Event::MergeStarted,
        "merge-started",
        EvidenceInput {
            evidence_id: format!("evidence-merge-preflight-{effect_id}"),
            kind: "GITHUB_MERGE_PREFLIGHT".into(),
            source: format!("github-pr-{}", case.pr_number.expect("validated PR")),
            payload: json!({"pull_request": after, "review_threads": after_threads}),
        },
    )?;
    if let Err(error) = LedgerController::apply(store, &policy.case_policy(), &command) {
        return Err(error.into());
    }
    Ok(MergeCycle::Prepared {
        case_key: case.case_key.clone(),
        head_sha: case.head_sha.clone().expect("validated head"),
    })
}

#[allow(clippy::too_many_arguments)]
fn execute<S: MergeSource, W: MergeWriter>(
    source: &S,
    writer: &W,
    policy: &RepositoryPolicy,
    store: &mut Store,
    case: &StoredCase,
    effect_id: &str,
    now: u64,
    owner: &str,
) -> Result<MergeCycle, MergeCycleError> {
    let (before, threads) = read_gate(source, policy, case)?;
    let merge_sha = if before.pull_request.merged {
        before
            .pull_request
            .merge_commit_sha
            .clone()
            .filter(|sha| GitSha::from_str(sha).is_ok())
            .ok_or(MergeCycleError::InvalidCase)?
    } else {
        let blockers = final_gate_blockers(store, case, policy, &before, &threads, Some(false))?;
        if !blockers.is_empty() {
            store.release_effect(effect_id, owner)?;
            return Ok(MergeCycle::Pending {
                case_key: case.case_key.clone(),
                blockers,
            });
        }
        let result = writer.merge(
            &MergeSpec {
                owner: policy.repository.owner.clone(),
                repository: policy.repository.name.clone(),
                pull_request_number: case.pr_number.ok_or(MergeCycleError::InvalidCase)?,
                expected_head_sha: case.head_sha.clone().ok_or(MergeCycleError::InvalidCase)?,
                commit_title: format!("Pip: resolve issue #{}", case.issue_number),
                method: policy.merge.method.clone(),
            },
            MergeModePolicy {
                guarded: true,
                autonomous_merge: true,
            },
        );
        match result {
            Ok(MutationResult::Merged(sha)) => sha,
            Ok(_) => {
                return Err(MergeCycleError::UnexpectedMutationResult);
            }
            Err(error) => {
                return Err(error.into());
            }
        }
    };
    let (after, _) = read_gate(source, policy, case)?;
    if !after.pull_request.merged
        || after.pull_request.open
        || after.pull_request.head_sha != case.head_sha.as_deref().unwrap_or_default()
        || after.pull_request.merge_commit_sha.as_deref() != Some(merge_sha.as_str())
    {
        return Err(MergeCycleError::InvalidCase);
    }
    let command = workflow(
        case,
        now,
        Event::MergeVerified,
        "merge-verified",
        EvidenceInput {
            evidence_id: format!("evidence-merge-result-{effect_id}"),
            kind: "GITHUB_MERGE_RESULT".into(),
            source: format!("github-pr-{}", case.pr_number.expect("validated PR")),
            payload: json!({"merge_commit_sha": merge_sha, "pull_request": after}),
        },
    )?;
    if let Err(error) = LedgerController::apply(store, &policy.case_policy(), &command) {
        return Err(error.into());
    }
    Ok(MergeCycle::Merged {
        case_key: case.case_key.clone(),
        merge_commit_sha: merge_sha,
    })
}

fn read_gate<S: MergeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    case: &StoredCase,
) -> Result<(PullRequestEvidence, Vec<ReviewThreadSnapshot>), MergeCycleError> {
    let number = case.pr_number.ok_or(MergeCycleError::InvalidCase)?;
    Ok((
        source.pull_request(
            &policy.repository.owner,
            &policy.repository.name,
            policy.repository.id,
            number,
        )?,
        source.review_threads(
            &policy.repository.owner,
            &policy.repository.name,
            policy.repository.id,
            number,
        )?,
    ))
}

fn has_final_ready(store: &Store, case: &StoredCase) -> Result<bool, MergeCycleError> {
    let pr = case.pr_number.ok_or(MergeCycleError::InvalidCase)?;
    let head = case
        .head_sha
        .as_deref()
        .ok_or(MergeCycleError::InvalidCase)?;
    let mut count = 0;
    for run in store.runs_for_case(&case.case_key)? {
        let result: WorkerResult =
            serde_json::from_value(run.payload).map_err(|_| MergeCycleError::InvalidCase)?;
        if let WorkerResult::Final(final_result) = result
            && final_result.outcome == FinalOutcome::Ready
            && final_result.plan_version == case.plan_version
            && final_result.pr_number == pr
            && final_result.reviewed_head_sha == head
        {
            count += 1;
        }
    }
    Ok(count == 1)
}

fn expected_branch(policy: &RepositoryPolicy, case: &StoredCase) -> String {
    format!(
        "{}repo-{}/issue-{}/workflow-{}",
        policy.branch_prefix, case.repository_id, case.issue_number, case.workflow_version
    )
}

fn workflow(
    case: &StoredCase,
    now: u64,
    event: Event,
    suffix: &str,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, MergeCycleError> {
    Ok(WorkflowCommand {
        case_id: CaseId::new(
            RepositoryId::new(
                NonZeroU64::new(case.repository_id).ok_or(MergeCycleError::InvalidCase)?,
            ),
            IssueNumber::new(
                NonZeroU64::new(case.issue_number).ok_or(MergeCycleError::InvalidCase)?,
            ),
            WorkflowVersion::new(
                NonZeroU32::new(case.workflow_version).ok_or(MergeCycleError::InvalidCase)?,
            ),
        ),
        event_id: EventId::from_str(&format!(
            "event-{suffix}-repo{}-issue{}-workflow{}-revision{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| MergeCycleError::InvalidCase)?,
        observed_at: ObservedAt::new(now),
        expected_state: CaseState::from_str(&case.state)
            .map_err(|_| MergeCycleError::InvalidCase)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(MergeCycleError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(MergeCycleError::InvalidCase)?,
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
            .map_err(|_| MergeCycleError::InvalidCase)?,
        event,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: evidence.payload.clone(),
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}
