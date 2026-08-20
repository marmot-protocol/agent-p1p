//! Policy-driven GitHub intake committed to the authoritative ledger.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

use pip_core::{
    ActorId, CaseId, CaseState, IntakeDecision, IssueNumber, IssueObservation, RepositoryId,
    WorkflowVersion, evaluate_intake,
};
use pip_store::{ApplyResult, EffectInput, EventInput, NewCase, PolicyInput, Store, StoreError};
use serde::Serialize;
use serde_json::json;

use crate::{IntakeSource, RepositoryPolicy, ShadowError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntakeCandidateResult {
    pub issue_number: u64,
    pub issue_id: u64,
    pub decision: String,
    pub blockers: Vec<String>,
    pub case_key: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActiveIntakeReport {
    pub report_format: u32,
    pub observed_at: u64,
    pub repository_id: u64,
    pub repository: String,
    pub policy_revision: u64,
    pub mutation_count: u64,
    pub candidates: Vec<IntakeCandidateResult>,
}

#[derive(Debug)]
pub enum ActiveIntakeError {
    ActivationDisabled,
    Evidence(ShadowError),
    Store(StoreError),
    Serialization(String),
    InvalidIdentity,
}

impl fmt::Display for ActiveIntakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActivationDisabled => {
                formatter.write_str("active intake requires enabled, unpaused intake and dispatch")
            }
            Self::Evidence(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Serialization(error) => write!(formatter, "intake serialization failed: {error}"),
            Self::InvalidIdentity => formatter.write_str("intake evidence has an invalid identity"),
        }
    }
}

impl std::error::Error for ActiveIntakeError {}

impl From<StoreError> for ActiveIntakeError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub fn reconcile_intake<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    observed_at: u64,
    global_paused: bool,
) -> Result<ActiveIntakeReport, ActiveIntakeError> {
    if !policy.intake.enabled || policy.intake.paused || !policy.dispatch_enabled || global_paused {
        return Err(ActiveIntakeError::ActivationDisabled);
    }
    let mut discovered = source
        .discover(
            &policy.repository.owner,
            &policy.repository.name,
            &policy.intake.label,
        )
        .map_err(|error| ActiveIntakeError::Evidence(ShadowError::Evidence(error.to_string())))?;
    discovered.sort_by_key(|issue| issue.number);
    let mut validated_evidence = Vec::with_capacity(discovered.len());
    for issue in discovered {
        let evidence = source
            .intake(
                &policy.repository.owner,
                &policy.repository.name,
                issue.number,
            )
            .map_err(|error| {
                ActiveIntakeError::Evidence(ShadowError::Evidence(error.to_string()))
            })?;
        if evidence.repository.id != policy.repository.id
            || evidence.repository.full_name != policy.repository.full_name()
            || evidence.repository.default_branch != policy.repository.default_branch
        {
            return Err(ActiveIntakeError::Evidence(ShadowError::RepositoryDrift));
        }
        if evidence.issue != issue {
            return Err(ActiveIntakeError::Evidence(ShadowError::DiscoveryDrift));
        }
        validated_evidence.push(evidence);
    }
    let policy_value = serde_json::to_value(policy)
        .map_err(|error| ActiveIntakeError::Serialization(error.to_string()))?;
    let policy_result = store.record_policy(&PolicyInput {
        repository_id: policy.repository.id,
        revision: policy.revision,
        accepted_at: observed_at,
        payload: policy_value,
    })?;
    let mut mutation_count = u64::from(policy_result == ApplyResult::Applied);
    let mut candidates = Vec::with_capacity(validated_evidence.len());
    for evidence in validated_evidence {
        let issue = evidence.issue.clone();
        let latest_event = evidence
            .label_events
            .iter()
            .filter(|event| event.label == policy.intake.label)
            .max_by_key(|event| (&event.created_at, event.id));
        let latest_label_actor_id = latest_event
            .filter(|event| event.labeled)
            .map(|event| event.actor_id);
        let case_id = case_id(policy, issue.number)?;
        let case_key = case_id.to_string();
        let status = store.status(observed_at)?;
        let repository_active_cases = status
            .cases
            .iter()
            .filter(|case| case.repository_id == policy.repository.id && active_state(&case.state))
            .count();
        let global_active_cases = status
            .cases
            .iter()
            .filter(|case| active_state(&case.state))
            .count();
        let observation = IssueObservation {
            open: evidence.issue.open,
            is_pull_request: evidence.issue.is_pull_request,
            labels: evidence.issue.labels.clone(),
            latest_label_actor_id: latest_label_actor_id
                .and_then(NonZeroU64::new)
                .map(ActorId::new),
            excluded: policy.intake.excluded_issue_numbers.contains(&issue.number),
            held: false,
            already_owned: store.case(&case_key)?.is_some(),
            repository_active_cases: u32::try_from(repository_active_cases).unwrap_or(u32::MAX),
            global_active_cases: u32::try_from(global_active_cases).unwrap_or(u32::MAX),
        };
        match evaluate_intake(&policy.intake_policy(global_paused), &observation) {
            IntakeDecision::Ineligible(blockers) => candidates.push(IntakeCandidateResult {
                issue_number: issue.number,
                issue_id: issue.id,
                decision: "INELIGIBLE".into(),
                blockers: blockers
                    .into_iter()
                    .map(|blocker| blocker.to_string())
                    .collect(),
                case_key: None,
            }),
            IntakeDecision::Eligible => {
                let label_event = latest_event
                    .filter(|event| event.labeled)
                    .ok_or(ActiveIntakeError::InvalidIdentity)?;
                let identity = format!(
                    "repo{}-issue{}-workflow{}-label{}",
                    policy.repository.id, issue.number, policy.workflow_version, label_event.id
                );
                let result = store.create_case(&NewCase {
                    case_key: case_key.clone(),
                    repository_id: policy.repository.id,
                    issue_number: issue.number,
                    workflow_version: policy.workflow_version,
                    policy_revision: policy.revision,
                    initial_state: CaseState::Planning.to_string(),
                    observed_at,
                    event: EventInput {
                        event_id: format!("event-intake-{identity}"),
                        event_type: "ISSUE_AUTHORIZED".into(),
                        payload: json!({
                            "repository_id": policy.repository.id,
                            "issue_id": issue.id,
                            "issue_number": issue.number,
                            "label": policy.intake.label,
                            "label_event_id": label_event.id,
                            "label_actor_id": label_event.actor_id,
                        }),
                    },
                    effects: vec![EffectInput {
                        effect_id: format!("effect-intake-{identity}-planner"),
                        effect_type: "DISPATCH_PLANNER".into(),
                        payload: json!({
                            "case_key": case_key,
                            "state_revision": 1,
                            "effect": "DISPATCH_PLANNER",
                        }),
                    }],
                })?;
                mutation_count += u64::from(result == ApplyResult::Applied);
                candidates.push(IntakeCandidateResult {
                    issue_number: issue.number,
                    issue_id: issue.id,
                    decision: "ELIGIBLE".into(),
                    blockers: Vec::new(),
                    case_key: Some(case_id.to_string()),
                });
            }
        }
    }
    Ok(ActiveIntakeReport {
        report_format: 1,
        observed_at,
        repository_id: policy.repository.id,
        repository: policy.repository.full_name(),
        policy_revision: policy.revision,
        mutation_count,
        candidates,
    })
}

fn case_id(policy: &RepositoryPolicy, issue_number: u64) -> Result<CaseId, ActiveIntakeError> {
    Ok(CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(policy.repository.id).ok_or(ActiveIntakeError::InvalidIdentity)?,
        ),
        IssueNumber::new(NonZeroU64::new(issue_number).ok_or(ActiveIntakeError::InvalidIdentity)?),
        WorkflowVersion::new(
            NonZeroU32::new(policy.workflow_version).ok_or(ActiveIntakeError::InvalidIdentity)?,
        ),
    ))
}

fn active_state(state: &str) -> bool {
    !matches!(state, "COMPLETED" | "ABANDONED" | "TAKEN_OVER")
}
