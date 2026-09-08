//! Policy-driven GitHub intake committed to the authoritative ledger.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

use pip_core::{
    ActorId, CaseId, CaseState, IntakeDecision, IssueNumber, IssueObservation, RepositoryId,
    WorkflowVersion, evaluate_intake,
};
use pip_store::{
    ApplyResult, EffectInput, EventInput, NewCase, PolicyInput, Store, StoreError,
    WebhookDeliveryInput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

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

#[derive(Clone, Copy, Debug)]
pub struct WebhookEnvelope<'a> {
    pub delivery_id: &'a str,
    pub event_name: &'a str,
    pub signature: &'a str,
    pub payload: &'a [u8],
    pub received_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WebhookIntakeReport {
    pub report_format: u32,
    pub observed_at: u64,
    pub repository_id: u64,
    pub repository: String,
    pub delivery_id: String,
    pub delivery: String,
    pub mutation_count: u64,
    pub candidate: Option<IntakeCandidateResult>,
}

#[derive(Debug)]
pub enum ActiveIntakeError {
    ActivationDisabled,
    Evidence(ShadowError),
    Store(StoreError),
    Serialization(String),
    InvalidIdentity,
    InvalidWebhook(&'static str),
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
            Self::InvalidWebhook(message) => write!(formatter, "invalid webhook: {message}"),
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
    // Webhook/standalone intake has no execution observation. Polling supplies
    // the Hermes quiescence check before reopening any existing case.
    reconcile_intake_with_quiescence(source, policy, store, observed_at, global_paused, |_| false)
}

pub fn reconcile_intake_with_quiescence<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    observed_at: u64,
    global_paused: bool,
    mut quiescent: impl FnMut(&[String]) -> bool,
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
    reconcile_validated_evidence(
        policy,
        store,
        observed_at,
        global_paused,
        validated_evidence,
        &mut quiescent,
    )
}

pub fn ingest_webhook<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    envelope: WebhookEnvelope<'_>,
    secret: &[u8],
    observed_at: u64,
    global_paused: bool,
) -> Result<WebhookIntakeReport, ActiveIntakeError> {
    if secret.is_empty() || secret.len() > 1024 {
        return Err(ActiveIntakeError::InvalidWebhook("invalid secret"));
    }
    if envelope.payload.is_empty() || envelope.payload.len() > 4 * 1024 * 1024 {
        return Err(ActiveIntakeError::InvalidWebhook("invalid payload size"));
    }
    if envelope.received_at == 0 || envelope.received_at > observed_at {
        return Err(ActiveIntakeError::InvalidWebhook("invalid receive time"));
    }
    if envelope.event_name != "issues"
        || !pip_github::verify_webhook(secret, envelope.payload, envelope.signature)
    {
        return Err(ActiveIntakeError::InvalidWebhook(
            "event or signature rejected",
        ));
    }
    let payload: IssuesWebhook = serde_json::from_slice(envelope.payload)
        .map_err(|_| ActiveIntakeError::InvalidWebhook("malformed issues event"))?;
    if payload.repository.id != policy.repository.id
        || payload.repository.full_name != policy.repository.full_name()
        || payload.issue.id == 0
        || payload.issue.number == 0
        || payload.action.is_empty()
        || payload.sender.id == 0
    {
        return Err(ActiveIntakeError::InvalidWebhook(
            "event is not an eligible repository label event",
        ));
    }
    let relevant_label = if payload.action == "labeled" {
        let label = payload
            .label
            .as_ref()
            .ok_or(ActiveIntakeError::InvalidWebhook(
                "labeled event is missing its label",
            ))?;
        label.name == policy.intake.label
    } else {
        false
    };
    let digest = Sha256::digest(envelope.payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let delivery = store.record_webhook_delivery(&WebhookDeliveryInput {
        delivery_id: envelope.delivery_id.into(),
        repository_id: policy.repository.id,
        event_name: envelope.event_name.into(),
        action: payload.action.clone(),
        received_at: envelope.received_at,
        payload_sha256: digest,
    })?;
    if !relevant_label {
        return Ok(WebhookIntakeReport {
            report_format: 1,
            observed_at,
            repository_id: policy.repository.id,
            repository: policy.repository.full_name(),
            delivery_id: envelope.delivery_id.into(),
            delivery: match delivery {
                ApplyResult::Applied => "APPLIED",
                ApplyResult::Replayed => "REPLAYED",
            }
            .into(),
            mutation_count: u64::from(delivery == ApplyResult::Applied),
            candidate: None,
        });
    }
    let evidence = source
        .intake(
            &policy.repository.owner,
            &policy.repository.name,
            payload.issue.number,
        )
        .map_err(|error| ActiveIntakeError::Evidence(ShadowError::Evidence(error.to_string())))?;
    let webhook_event_exists = evidence
        .label_events
        .iter()
        .filter(|event| event.label == policy.intake.label)
        .any(|event| event.labeled && event.actor_id == payload.sender.id);
    if evidence.repository.id != policy.repository.id
        || evidence.repository.full_name != policy.repository.full_name()
        || evidence.repository.default_branch != policy.repository.default_branch
        || evidence.issue.id != payload.issue.id
        || evidence.issue.number != payload.issue.number
        || !webhook_event_exists
    {
        return Err(ActiveIntakeError::Evidence(ShadowError::DiscoveryDrift));
    }
    let report = reconcile_validated_evidence(
        policy,
        store,
        observed_at,
        global_paused,
        vec![evidence],
        &mut |_| false,
    )?;
    Ok(WebhookIntakeReport {
        report_format: 1,
        observed_at,
        repository_id: policy.repository.id,
        repository: policy.repository.full_name(),
        delivery_id: envelope.delivery_id.into(),
        delivery: match delivery {
            ApplyResult::Applied => "APPLIED",
            ApplyResult::Replayed => "REPLAYED",
        }
        .into(),
        mutation_count: report.mutation_count + u64::from(delivery == ApplyResult::Applied),
        candidate: report.candidates.into_iter().next(),
    })
}

#[derive(Deserialize)]
struct IssuesWebhook {
    action: String,
    repository: WebhookRepository,
    issue: WebhookIssue,
    label: Option<WebhookLabel>,
    sender: WebhookSender,
}

#[derive(Deserialize)]
struct WebhookRepository {
    id: u64,
    full_name: String,
}

#[derive(Deserialize)]
struct WebhookIssue {
    id: u64,
    number: u64,
}

#[derive(Deserialize)]
struct WebhookLabel {
    name: String,
}

#[derive(Deserialize)]
struct WebhookSender {
    id: u64,
}

fn reconcile_validated_evidence(
    policy: &RepositoryPolicy,
    store: &mut Store,
    observed_at: u64,
    global_paused: bool,
    validated_evidence: Vec<pip_github::IntakeSnapshot>,
    quiescent: &mut impl FnMut(&[String]) -> bool,
) -> Result<ActiveIntakeReport, ActiveIntakeError> {
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
        let existing = store.case(&case_key)?;
        let removed_label_id = evidence
            .label_events
            .iter()
            .filter(|event| {
                event.label == policy.intake.label
                    && !event.labeled
                    && latest_event.is_some_and(|latest| event.id < latest.id)
            })
            .map(|event| event.id)
            .max()
            .unwrap_or(0);
        let reauthorize = if existing.is_some() {
            store.can_reauthorize(
                &case_key,
                latest_event.map_or(0, |event| event.id),
                removed_label_id,
            )?
        } else {
            false
        };
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
            held: policy.intake.held_issue_numbers.contains(&issue.number),
            already_owned: existing.is_some() && !reauthorize,
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
                if reauthorize && !quiescent(&store.case_task_ids(&case_key)?) {
                    candidates.push(IntakeCandidateResult {
                        issue_number: issue.number,
                        issue_id: issue.id,
                        decision: "INELIGIBLE".into(),
                        blockers: vec!["PREVIOUS_WORK_NOT_QUIESCENT".into()],
                        case_key: None,
                    });
                    continue;
                }
                // Freeze the controller's authenticated read with the authorization
                // event. Workers receive this through the digest-bound history bundle,
                // without GitHub credentials or a second live-read implementation.
                let issue_context = json!({
                    "schema_version": 1,
                    "observed_at": observed_at,
                    "repository": evidence.repository,
                    "issue": evidence.issue,
                    "issue_content": evidence.issue_content,
                    "comments": evidence.comments,
                });
                let context_bytes = serde_json::to_vec(&issue_context)
                    .map_err(|error| ActiveIntakeError::Serialization(error.to_string()))?;
                if context_bytes.len() > 256 * 1024 {
                    return Err(ActiveIntakeError::InvalidWebhook(
                        "issue context exceeds worker evidence bound",
                    ));
                }
                let label_event = latest_event
                    .filter(|event| event.labeled)
                    .ok_or(ActiveIntakeError::InvalidIdentity)?;
                let identity = format!(
                    "repo{}-issue{}-workflow{}-label{}",
                    policy.repository.id, issue.number, policy.workflow_version, label_event.id
                );
                let next_revision = existing.as_ref().map_or(1, |case| case.state_revision + 1);
                let mut input = NewCase {
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
                            "issue_context": issue_context,
                        }),
                    },
                    effects: vec![EffectInput {
                        effect_id: format!("effect-intake-{identity}-planner"),
                        effect_type: "DISPATCH_PLANNER".into(),
                        payload: json!({
                            "case_key": case_key,
                            "state_revision": next_revision,
                            "effect": "DISPATCH_PLANNER",
                        }),
                    }],
                };
                let result = if let Some(case) = existing {
                    input.event.event_type = "ISSUE_REAUTHORIZED".into();
                    input.event.payload["removed_label_event_id"] = json!(removed_label_id);
                    input.event.payload["policy_revision"] = json!(policy.revision);
                    store.apply_transition(
                        &pip_store::TransitionInput {
                            case_key: case_key.clone(),
                            expected_revision: case.state_revision,
                            next_state: "PLANNING".into(),
                            remediation_round: case.remediation_round,
                            plan_version: case.plan_version,
                            pr_number: None,
                            head_sha: None,
                            observed_at,
                            event: input.event,
                            run: None,
                            evidence: vec![],
                            findings: vec![],
                            effects: input.effects,
                        },
                        None,
                    )?
                } else {
                    store.create_case(&input)?
                };
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
