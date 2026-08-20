use std::fmt;
use std::num::NonZeroU32;

use crate::{
    CaseId, CaseState, Event, EventId, MergeMode, ObservedAt, PolicyRevision, StateRevision,
    TransitionContext, TransitionDecision, TransitionError, transition,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CasePolicy {
    pub revision: PolicyRevision,
    pub merge_mode: MergeMode,
    pub max_remediation_rounds: NonZeroU32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaseSnapshot {
    pub case_id: CaseId,
    pub state: CaseState,
    pub state_revision: StateRevision,
    pub policy_revision: PolicyRevision,
    pub remediation_round: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaseCommand {
    pub case_id: CaseId,
    pub event_id: EventId,
    pub observed_at: ObservedAt,
    pub expected_state_revision: StateRevision,
    pub accepted_policy_revision: PolicyRevision,
    pub event: Event,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaseCommandDecision {
    pub case_id: CaseId,
    pub event_id: EventId,
    pub observed_at: ObservedAt,
    pub from_revision: StateRevision,
    pub to_revision: StateRevision,
    pub transition: TransitionDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandError {
    CaseMismatch,
    StaleStateRevision {
        expected: StateRevision,
        actual: StateRevision,
    },
    CommandPolicyRevisionMismatch,
    PolicyRevisionUnavailable,
    StateRevisionExhausted,
    InvalidTransition(TransitionError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CaseMismatch => formatter.write_str("command case identity mismatch"),
            Self::StaleStateRevision { expected, actual } => write!(
                formatter,
                "stale state revision: expected {}, actual {}",
                expected.get(),
                actual.get()
            ),
            Self::CommandPolicyRevisionMismatch => {
                formatter.write_str("command policy revision does not match the case")
            }
            Self::PolicyRevisionUnavailable => {
                formatter.write_str("accepted case policy revision is unavailable")
            }
            Self::StateRevisionExhausted => formatter.write_str("state revision exhausted"),
            Self::InvalidTransition(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CommandError {}

pub fn evaluate_case_command(
    snapshot: &CaseSnapshot,
    command: &CaseCommand,
    policy: &CasePolicy,
) -> Result<CaseCommandDecision, CommandError> {
    if command.case_id != snapshot.case_id {
        return Err(CommandError::CaseMismatch);
    }
    if command.expected_state_revision != snapshot.state_revision {
        return Err(CommandError::StaleStateRevision {
            expected: command.expected_state_revision,
            actual: snapshot.state_revision,
        });
    }
    if command.accepted_policy_revision != snapshot.policy_revision {
        return Err(CommandError::CommandPolicyRevisionMismatch);
    }
    if policy.revision != snapshot.policy_revision {
        return Err(CommandError::PolicyRevisionUnavailable);
    }
    let to_revision = snapshot
        .state_revision
        .checked_next()
        .ok_or(CommandError::StateRevisionExhausted)?;
    let transition = transition(
        snapshot.state,
        command.event,
        TransitionContext {
            remediation_round: snapshot.remediation_round,
            max_remediation_rounds: policy.max_remediation_rounds.get(),
            merge_mode: policy.merge_mode,
        },
    )
    .map_err(CommandError::InvalidTransition)?;

    Ok(CaseCommandDecision {
        case_id: snapshot.case_id,
        event_id: command.event_id.clone(),
        observed_at: command.observed_at,
        from_revision: snapshot.state_revision,
        to_revision,
        transition,
    })
}
