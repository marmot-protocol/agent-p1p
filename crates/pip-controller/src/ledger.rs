//! Pure-core decisions committed through the authoritative ledger.

use std::fmt;

use pip_core::{
    CaseCommand, CasePolicy, CaseSnapshot, CaseState, CommandError, Effect, Event, EventId, GitSha,
    ObservedAt, PlanVersion, PolicyRevision, PullRequestNumber, StateRevision,
    evaluate_case_command,
};
use pip_store::{
    ApplyResult, EffectInput, EventInput, EvidenceInput, FindingInput, RunInput, Store, StoreError,
    TransitionInput,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct WorkflowCommand {
    pub case_id: pip_core::CaseId,
    pub event_id: EventId,
    pub observed_at: ObservedAt,
    pub expected_state: CaseState,
    pub expected_state_revision: StateRevision,
    pub accepted_policy_revision: PolicyRevision,
    pub remediation_round: u32,
    pub plan_version: Option<PlanVersion>,
    pub pr_number: Option<PullRequestNumber>,
    pub head_sha: Option<GitSha>,
    pub event: Event,
    pub accepted_plan_version: Option<PlanVersion>,
    pub next_pr_number: Option<PullRequestNumber>,
    pub next_head_sha: Option<GitSha>,
    pub event_payload: Value,
    pub run: Option<RunInput>,
    pub evidence: Vec<EvidenceInput>,
    pub findings: Vec<FindingInput>,
}

#[derive(Debug)]
pub enum ControllerError {
    InvalidCommand(&'static str),
    Core(CommandError),
    Store(StoreError),
}

impl fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCommand(message) => formatter.write_str(message),
            Self::Core(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ControllerError {}

impl From<CommandError> for ControllerError {
    fn from(error: CommandError) -> Self {
        Self::Core(error)
    }
}

impl From<StoreError> for ControllerError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub struct LedgerController;

impl LedgerController {
    pub fn apply(
        store: &mut Store,
        policy: &CasePolicy,
        workflow: &WorkflowCommand,
    ) -> Result<ApplyResult, ControllerError> {
        validate_workflow(store, workflow)?;
        let transition = transition_input(policy, workflow)?;
        store
            .apply_transition(&transition, None)
            .map_err(Into::into)
    }

    pub fn apply_batch(
        store: &mut Store,
        policy: &CasePolicy,
        workflows: &[WorkflowCommand],
    ) -> Result<ApplyResult, ControllerError> {
        let first = workflows.first().ok_or(ControllerError::InvalidCommand(
            "at least one workflow command is required",
        ))?;
        validate_workflow(store, first)?;
        let mut transitions = Vec::with_capacity(workflows.len());
        for workflow in workflows {
            validate_workflow_shape(workflow)?;
            if let Some(previous) = transitions.last() {
                validate_consecutive(previous, workflow)?;
            }
            transitions.push(transition_input(policy, workflow)?);
        }
        store
            .apply_transition_batch(&transitions, None)
            .map_err(Into::into)
    }
}

fn transition_input(
    policy: &CasePolicy,
    workflow: &WorkflowCommand,
) -> Result<TransitionInput, ControllerError> {
    let snapshot = CaseSnapshot {
        case_id: workflow.case_id,
        state: workflow.expected_state,
        state_revision: workflow.expected_state_revision,
        policy_revision: workflow.accepted_policy_revision,
        remediation_round: workflow.remediation_round,
    };
    let command = CaseCommand {
        case_id: workflow.case_id,
        event_id: workflow.event_id.clone(),
        observed_at: workflow.observed_at,
        expected_state_revision: workflow.expected_state_revision,
        accepted_policy_revision: workflow.accepted_policy_revision,
        event: workflow.event,
    };
    let evaluated = evaluate_case_command(&snapshot, &command, policy)?;
    let next_plan_version = next_plan_version(workflow)?;
    let next_remediation_round = if evaluated.transition.next_state == CaseState::Remediating
        && workflow.expected_state != CaseState::Remediating
    {
        workflow
            .remediation_round
            .checked_add(1)
            .ok_or(ControllerError::InvalidCommand(
                "remediation round exhausted",
            ))?
    } else {
        workflow.remediation_round
    };
    let (next_pr_number, next_head_sha) = next_head_binding(workflow)?;
    let effects = evaluated
        .transition
        .effects
        .iter()
        .enumerate()
        .map(|(index, effect)| EffectInput {
            effect_id: format!(
                "effect-{}-{}-{}",
                workflow.event_id,
                effect_name(*effect).to_ascii_lowercase(),
                index + 1
            ),
            effect_type: effect_name(*effect).into(),
            payload: json!({
                "case_key": workflow.case_id.to_string(),
                "state_revision": evaluated.to_revision.get(),
                "effect": effect_name(*effect),
                "remediation_round": next_remediation_round,
                "plan_version": next_plan_version,
                "pr_number": next_pr_number,
                "head_sha": next_head_sha,
            }),
        })
        .collect();
    Ok(TransitionInput {
        case_key: workflow.case_id.to_string(),
        expected_revision: workflow.expected_state_revision.get(),
        next_state: evaluated.transition.next_state.to_string(),
        remediation_round: next_remediation_round,
        plan_version: next_plan_version,
        pr_number: next_pr_number,
        head_sha: next_head_sha,
        observed_at: workflow.observed_at.unix_seconds(),
        event: EventInput {
            event_id: workflow.event_id.to_string(),
            event_type: workflow.event.to_string(),
            payload: workflow.event_payload.clone(),
        },
        run: workflow.run.clone(),
        evidence: workflow.evidence.clone(),
        findings: workflow.findings.clone(),
        effects,
    })
}

fn validate_consecutive(
    previous: &TransitionInput,
    workflow: &WorkflowCommand,
) -> Result<(), ControllerError> {
    let expected_revision =
        previous
            .expected_revision
            .checked_add(1)
            .ok_or(ControllerError::InvalidCommand(
                "workflow revision overflow",
            ))?;
    if workflow.case_id.to_string() != previous.case_key
        || workflow.expected_state_revision.get() != expected_revision
        || workflow.expected_state.to_string() != previous.next_state
        || workflow.remediation_round != previous.remediation_round
        || workflow.plan_version.map_or(0, PlanVersion::get) != previous.plan_version
        || workflow.pr_number.map(PullRequestNumber::get) != previous.pr_number
        || workflow.head_sha.map(|sha| sha.to_string()) != previous.head_sha
    {
        return Err(ControllerError::InvalidCommand(
            "workflow batch is not consecutive",
        ));
    }
    Ok(())
}

fn validate_workflow(store: &Store, workflow: &WorkflowCommand) -> Result<(), ControllerError> {
    validate_workflow_shape(workflow)?;
    let stored = store
        .case(&workflow.case_id.to_string())?
        .ok_or(ControllerError::InvalidCommand("case is not in the ledger"))?;
    if stored.state_revision == workflow.expected_state_revision.get() {
        let plan = workflow.plan_version.map_or(0, PlanVersion::get);
        let pr = workflow.pr_number.map(PullRequestNumber::get);
        let head = workflow.head_sha.map(|sha| sha.to_string());
        if stored.state != workflow.expected_state.to_string()
            || stored.policy_revision != workflow.accepted_policy_revision.get()
            || stored.remediation_round != workflow.remediation_round
            || stored.plan_version != plan
            || stored.pr_number != pr
            || stored.head_sha != head
        {
            return Err(ControllerError::InvalidCommand(
                "workflow command does not match the ledger projection",
            ));
        }
    }
    Ok(())
}

fn validate_workflow_shape(workflow: &WorkflowCommand) -> Result<(), ControllerError> {
    if !workflow.event_payload.is_object() {
        return Err(ControllerError::InvalidCommand(
            "workflow event payload must be an object",
        ));
    }
    if workflow.pr_number.is_some() != workflow.head_sha.is_some()
        || workflow.next_pr_number.is_some() && workflow.next_head_sha.is_none()
    {
        return Err(ControllerError::InvalidCommand(
            "pull request and exact head bindings must be complete",
        ));
    }
    Ok(())
}

fn next_plan_version(workflow: &WorkflowCommand) -> Result<u32, ControllerError> {
    let current = workflow.plan_version.map_or(0, PlanVersion::get);
    match workflow.accepted_plan_version {
        Some(accepted) if accepted.get() == current.saturating_add(1) => Ok(accepted.get()),
        Some(_) => Err(ControllerError::InvalidCommand(
            "accepted plan version is not the next version",
        )),
        None if workflow.event == Event::Proceed => Err(ControllerError::InvalidCommand(
            "planner proceed requires an accepted plan version",
        )),
        None => Ok(current),
    }
}

fn next_head_binding(
    workflow: &WorkflowCommand,
) -> Result<(Option<u64>, Option<String>), ControllerError> {
    if workflow.event == Event::ReviewReady {
        let head = workflow
            .next_head_sha
            .ok_or(ControllerError::InvalidCommand(
                "review-ready builder result requires a new exact head",
            ))?;
        let pr = workflow.next_pr_number.or(workflow.pr_number).ok_or(
            ControllerError::InvalidCommand("review-ready builder result requires a pull request"),
        )?;
        if workflow.pr_number.is_some_and(|current| current != pr) {
            return Err(ControllerError::InvalidCommand(
                "builder cannot replace the case pull request",
            ));
        }
        return Ok((Some(pr.get()), Some(head.to_string())));
    }
    if workflow.next_pr_number.is_some() || workflow.next_head_sha.is_some() {
        return Err(ControllerError::InvalidCommand(
            "only review-ready builder evidence may change the exact head",
        ));
    }
    Ok((
        workflow.pr_number.map(PullRequestNumber::get),
        workflow.head_sha.map(|head| head.to_string()),
    ))
}

fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::DispatchPlanner => "DISPATCH_PLANNER",
        Effect::PublishPlan => "PUBLISH_PLAN",
        Effect::DispatchBuilder => "DISPATCH_BUILDER",
        Effect::PublishDraftPullRequest => "PUBLISH_DRAFT_PULL_REQUEST",
        Effect::ObserveCi => "OBSERVE_CI",
        Effect::DispatchReviewers => "DISPATCH_REVIEWERS",
        Effect::PublishReviews => "PUBLISH_REVIEWS",
        Effect::ObserveFinalPreflight => "OBSERVE_FINAL_PREFLIGHT",
        Effect::DispatchFinalReviewer => "DISPATCH_FINAL_REVIEWER",
        Effect::HoldForHuman => "HOLD_FOR_HUMAN",
        Effect::NotifyShadowReady => "NOTIFY_SHADOW_READY",
        Effect::BeginMerge => "BEGIN_MERGE",
        Effect::ExecuteMerge => "EXECUTE_MERGE",
        Effect::RecordCompletion => "RECORD_COMPLETION",
        Effect::RecordAbandonment => "RECORD_ABANDONMENT",
        Effect::RecordBlock => "RECORD_BLOCK",
        Effect::RecordTakeover => "RECORD_TAKEOVER",
        Effect::Escalate => "ESCALATE",
    }
}
