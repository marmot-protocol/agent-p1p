//! Immutable worker-result validation, review joins, and ledger transitions.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{
    BuilderOutcome, ContractError, FinalOutcome, PlannerOutcome, ReviewOutcome, ReviewResult,
    WorkerBinding, WorkerResult, WorkerRole,
};
use pip_core::{
    CaseId, CasePolicy, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_store::{EvidenceInput, FindingInput, RunInput, Store, StoreError, StoredCase};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{ControllerError, LedgerController, WorkflowCommand};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestResult {
    Applied { transition_count: u32 },
    Replayed,
}

#[derive(Debug)]
pub enum IngestError {
    Contract(ContractError),
    Store(StoreError),
    Controller(ControllerError),
    InvalidBinding,
    InvalidState,
    ConflictingRun,
    ConflictingReviews,
    Serialization(String),
}

impl fmt::Display for IngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::InvalidBinding => formatter.write_str("worker binding does not name this case"),
            Self::InvalidState => formatter.write_str("worker result is invalid for case state"),
            Self::ConflictingRun => formatter.write_str("task already has a different run"),
            Self::ConflictingReviews => {
                formatter.write_str("independent review results conflict or duplicate a role")
            }
            Self::Serialization(error) => {
                write!(formatter, "worker result serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for IngestError {}

impl From<ContractError> for IngestError {
    fn from(error: ContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<StoreError> for IngestError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ControllerError> for IngestError {
    fn from(error: ControllerError) -> Self {
        Self::Controller(error)
    }
}

pub fn ingest_worker_result(
    store: &mut Store,
    policy: &CasePolicy,
    binding: &WorkerBinding,
    result: &WorkerResult,
) -> Result<IngestResult, IngestError> {
    result.validate_binding(binding)?;
    let payload = serde_json::to_value(result)
        .map_err(|error| IngestError::Serialization(error.to_string()))?;
    if let Some(existing) = store.run_by_task_id(&binding.task_id)? {
        if existing.case_key == case_id(binding)?.to_string()
            && existing.role == role_name(binding.role)
            && existing.payload == payload
        {
            return Ok(IngestResult::Replayed);
        }
        return Err(IngestError::ConflictingRun);
    }

    let case_id = case_id(binding)?;
    let stored = store
        .case(&case_id.to_string())?
        .ok_or(IngestError::InvalidBinding)?;
    validate_case_binding(&stored, binding, policy)?;
    if matches!(result, WorkerResult::Builder(_)) && stored.state == "READY_TO_BUILD" {
        let synthetic = workflow(
            &stored,
            case_id,
            Event::BuilderDispatched,
            &result_event_id("dispatch", &binding.task_id),
            result.common().started_at_unix,
            json!({"task_id": binding.task_id, "role": "builder"}),
            None,
            None,
            None,
            Vec::new(),
        )?;
        let mut building = stored.clone();
        building.state = "BUILDING".into();
        building.state_revision = building
            .state_revision
            .checked_add(1)
            .ok_or(IngestError::InvalidState)?;
        let result_command = result_workflow(store, &building, case_id, binding, result, payload)?;
        LedgerController::apply_batch(store, policy, &[synthetic, result_command])?;
        return Ok(IngestResult::Applied {
            transition_count: 2,
        });
    }

    let command = result_workflow(store, &stored, case_id, binding, result, payload)?;
    LedgerController::apply(store, policy, &command)?;
    Ok(IngestResult::Applied {
        transition_count: 1,
    })
}

fn result_workflow(
    store: &Store,
    stored: &StoredCase,
    case_id: CaseId,
    binding: &WorkerBinding,
    result: &WorkerResult,
    payload: Value,
) -> Result<WorkflowCommand, IngestError> {
    let mapped = map_event(store, stored, result)?;
    let findings = findings(result)?;
    let run = RunInput {
        run_id: format!("run-{}", binding.task_id),
        task_id: binding.task_id.clone(),
        role: role_name(binding.role).into(),
        payload: payload.clone(),
    };
    let evidence = vec![EvidenceInput {
        evidence_id: format!("evidence-worker-{}", binding.task_id),
        kind: "WORKER_RESULT_EVIDENCE".into(),
        source: binding.task_id.clone(),
        payload: Value::Object(result.common().evidence.clone()),
    }];
    let command = workflow(
        stored,
        case_id,
        mapped.event,
        &result_event_id("result", &binding.task_id),
        result.common().completed_at_unix,
        payload,
        Some(run),
        mapped.accepted_plan_version,
        next_binding(mapped.next_pr_number, mapped.next_head_sha.as_deref()),
        evidence,
    )?;
    Ok(WorkflowCommand {
        findings,
        ..command
    })
}

struct MappedEvent {
    event: Event,
    accepted_plan_version: Option<u32>,
    next_pr_number: Option<u64>,
    next_head_sha: Option<String>,
}

fn mapped(
    event: Event,
    accepted_plan_version: Option<u32>,
    next_pr_number: Option<u64>,
    next_head_sha: Option<String>,
) -> MappedEvent {
    MappedEvent {
        event,
        accepted_plan_version,
        next_pr_number,
        next_head_sha,
    }
}

fn map_event(
    store: &Store,
    case: &StoredCase,
    result: &WorkerResult,
) -> Result<MappedEvent, IngestError> {
    match result {
        WorkerResult::Planner(result) if case.state == "PLANNING" => Ok(mapped(
            if result.outcome == PlannerOutcome::BlockedUnexpectedModel {
                Event::BlockedUnexpectedModel
            } else {
                Event::PlanRecorded
            },
            None,
            None,
            None,
        )),
        WorkerResult::Builder(result)
            if matches!(case.state.as_str(), "BUILDING" | "REMEDIATING") =>
        {
            Ok(mapped(
                match result.outcome {
                    BuilderOutcome::ReviewReady => Event::BuildRecorded,
                    BuilderOutcome::ReturnToPlanning => Event::ReturnToPlanning,
                    BuilderOutcome::Blocked => Event::Blocked,
                    BuilderOutcome::Abandon => Event::Abandon,
                    BuilderOutcome::BlockedUnexpectedModel => Event::BlockedUnexpectedModel,
                },
                None,
                None,
                None,
            ))
        }
        WorkerResult::Review(result) if case.state == "REVIEWING" => {
            let event = match result.outcome {
                ReviewOutcome::Blocked => Event::Blocked,
                ReviewOutcome::BlockedUnexpectedModel => Event::BlockedUnexpectedModel,
                ReviewOutcome::Approve | ReviewOutcome::RequestChanges => {
                    join_review(store, case, result)?
                }
            };
            Ok(mapped(event, None, None, None))
        }
        WorkerResult::Final(result)
            if case.state == "FINAL_REVIEW"
                && store.latest_event_type(&case.case_key)?.as_deref()
                    == Some("FINAL_PREFLIGHT_ACCEPTED") =>
        {
            Ok(mapped(
                match result.outcome {
                    FinalOutcome::Ready => Event::Ready,
                    FinalOutcome::ReturnToBuild => Event::ReturnToBuild,
                    FinalOutcome::ReturnToReview => Event::ReturnToReview,
                    FinalOutcome::ReturnToPlanning => Event::ReturnToPlanning,
                    FinalOutcome::WaitForIssueCreator => Event::WaitForIssueCreator,
                    FinalOutcome::Blocked => Event::Blocked,
                    FinalOutcome::Abandon => Event::Abandon,
                    FinalOutcome::BlockedUnexpectedModel => Event::BlockedUnexpectedModel,
                },
                None,
                None,
                None,
            ))
        }
        _ => Err(IngestError::InvalidState),
    }
}

fn join_review(
    store: &Store,
    case: &StoredCase,
    current: &ReviewResult,
) -> Result<Event, IngestError> {
    let mut matching = Vec::new();
    for stored in store.runs_for_case(&case.case_key)? {
        let parsed: WorkerResult = serde_json::from_value(stored.payload)
            .map_err(|error| IngestError::Serialization(error.to_string()))?;
        if let WorkerResult::Review(previous) = parsed
            && previous.plan_version == current.plan_version
            && previous.review_round == current.review_round
            && previous.pr_number == current.pr_number
            && previous.reviewed_head_sha == current.reviewed_head_sha
        {
            matching.push(previous);
        }
    }
    if matching
        .iter()
        .any(|prior| prior.common.role == current.common.role)
        || matching.len() > 1
    {
        return Err(IngestError::ConflictingReviews);
    }
    let Some(previous) = matching.first() else {
        return Ok(Event::ReviewRecorded);
    };
    if !matches!(
        previous.outcome,
        ReviewOutcome::Approve | ReviewOutcome::RequestChanges
    ) {
        return Err(IngestError::ConflictingReviews);
    }
    if previous.outcome == ReviewOutcome::RequestChanges
        || current.outcome == ReviewOutcome::RequestChanges
    {
        Ok(Event::RequestChanges)
    } else {
        Ok(Event::ReviewsApproved)
    }
}

fn findings(result: &WorkerResult) -> Result<Vec<FindingInput>, IngestError> {
    let WorkerResult::Review(result) = result else {
        return Ok(Vec::new());
    };
    result
        .blocking_findings
        .iter()
        .map(|finding| {
            Ok(FindingInput {
                finding_id: finding.id.clone(),
                origin_role: role_name(result.common.role).into(),
                reviewed_head_sha: result.reviewed_head_sha.clone(),
                payload: serde_json::to_value(finding)
                    .map_err(|error| IngestError::Serialization(error.to_string()))?,
            })
        })
        .collect()
}

fn validate_case_binding(
    case: &StoredCase,
    binding: &WorkerBinding,
    policy: &CasePolicy,
) -> Result<(), IngestError> {
    if case.repository_id != binding.case.repository_id
        || case.issue_number != binding.case.issue_number
        || case.workflow_version != binding.case.workflow_version
        || case.policy_revision != policy.revision.get()
    {
        return Err(IngestError::InvalidBinding);
    }
    Ok(())
}

fn case_id(binding: &WorkerBinding) -> Result<CaseId, IngestError> {
    Ok(CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(binding.case.repository_id).ok_or(IngestError::InvalidBinding)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(binding.case.issue_number).ok_or(IngestError::InvalidBinding)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(binding.case.workflow_version).ok_or(IngestError::InvalidBinding)?,
        ),
    ))
}

#[allow(clippy::too_many_arguments)]
fn workflow(
    case: &StoredCase,
    case_id: CaseId,
    event: Event,
    event_id: &str,
    observed_at: u64,
    payload: Value,
    run: Option<RunInput>,
    accepted_plan_version: Option<u32>,
    next_binding: Option<(u64, &str)>,
    evidence: Vec<EvidenceInput>,
) -> Result<WorkflowCommand, IngestError> {
    let (next_pr_number, next_head_sha) = next_binding.unzip();
    Ok(WorkflowCommand {
        case_id,
        event_id: EventId::from_str(event_id).map_err(|_| IngestError::InvalidBinding)?,
        observed_at: ObservedAt::new(observed_at),
        expected_state: CaseState::from_str(&case.state).map_err(|_| IngestError::InvalidState)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(IngestError::InvalidState)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(IngestError::InvalidState)?,
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
            .map_err(|_| IngestError::InvalidState)?,
        event,
        accepted_plan_version: accepted_plan_version
            .and_then(NonZeroU32::new)
            .map(PlanVersion::new),
        next_pr_number: next_pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        next_head_sha: next_head_sha
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| IngestError::InvalidBinding)?,
        event_payload: payload,
        run,
        evidence,
        findings: Vec::new(),
    })
}

fn next_binding(pr: Option<u64>, head: Option<&str>) -> Option<(u64, &str)> {
    pr.zip(head)
}

fn result_event_id(kind: &str, task_id: &str) -> String {
    let digest = Sha256::digest(task_id.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("event-{kind}-{hex}")
}

fn role_name(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Planner => "planner",
        WorkerRole::Builder => "builder",
        WorkerRole::ReviewerGeneral => "reviewer-general",
        WorkerRole::ReviewerSecperf => "reviewer-secperf",
        WorkerRole::FinalReviewer => "final-reviewer",
    }
}
