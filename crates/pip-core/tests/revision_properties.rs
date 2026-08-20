use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_core::{
    CaseCommand, CaseId, CasePolicy, CaseSnapshot, CaseState, CommandError, Event, EventId,
    IssueNumber, MergeMode, ObservedAt, PolicyRevision, RepositoryId, StateRevision,
    WorkflowVersion, evaluate_case_command,
};
use proptest::prelude::*;

fn case_id() -> CaseId {
    CaseId::new(
        RepositoryId::new(NonZeroU64::new(984_321).unwrap()),
        IssueNumber::new(NonZeroU64::new(1240).unwrap()),
        WorkflowVersion::new(NonZeroU32::new(2).unwrap()),
    )
}

fn policy() -> CasePolicy {
    CasePolicy {
        revision: PolicyRevision::new(NonZeroU64::new(3).unwrap()),
        merge_mode: MergeMode::Shadow,
        max_remediation_rounds: NonZeroU32::new(3).unwrap(),
    }
}

fn snapshot(revision: u64) -> CaseSnapshot {
    CaseSnapshot {
        case_id: case_id(),
        state: CaseState::Planning,
        state_revision: StateRevision::new(NonZeroU64::new(revision).unwrap()),
        policy_revision: policy().revision,
        remediation_round: 0,
    }
}

fn command(expected_revision: u64) -> CaseCommand {
    CaseCommand {
        case_id: case_id(),
        event_id: EventId::from_str("event-1").unwrap(),
        observed_at: ObservedAt::new(1_787_000_000),
        expected_state_revision: StateRevision::new(NonZeroU64::new(expected_revision).unwrap()),
        accepted_policy_revision: policy().revision,
        event: Event::Proceed,
    }
}

#[test]
fn stale_state_revision_fails_without_a_transition() {
    assert_eq!(
        evaluate_case_command(&snapshot(7), &command(6), &policy()),
        Err(CommandError::StaleStateRevision {
            expected: StateRevision::new(NonZeroU64::new(6).unwrap()),
            actual: StateRevision::new(NonZeroU64::new(7).unwrap()),
        })
    );
}

#[test]
fn accepted_command_carries_explicit_identity_time_and_next_revision() {
    let decision = evaluate_case_command(&snapshot(7), &command(7), &policy()).unwrap();
    assert_eq!(decision.case_id, case_id());
    assert_eq!(decision.event_id, EventId::from_str("event-1").unwrap());
    assert_eq!(decision.observed_at, ObservedAt::new(1_787_000_000));
    assert_eq!(decision.from_revision.get(), 7);
    assert_eq!(decision.to_revision.get(), 8);
    assert_eq!(decision.transition.next_state, CaseState::ReadyToBuild);
}

proptest! {
    #[test]
    fn replaying_the_same_pure_command_is_idempotent(revision in 1_u64..u64::MAX) {
        let snapshot = snapshot(revision);
        let command = command(revision);
        let first = evaluate_case_command(&snapshot, &command, &policy());
        let replay = evaluate_case_command(&snapshot, &command, &policy());
        prop_assert_eq!(first, replay);
    }
}
