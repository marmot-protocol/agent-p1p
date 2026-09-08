use pip_core::{CaseState as S, Effect, Event, TransitionContext, transition};

#[test]
fn human_feedback_replans_at_safe_handoff_without_reopening_terminal_or_operational_failures() {
    let event: Event = "HUMAN_FEEDBACK_RECEIVED".parse().unwrap();
    for state in [
        S::Planning,
        S::WaitingHuman,
        S::ReadyToBuild,
        S::WaitingCi,
        S::Reviewing,
        S::Remediating,
        S::FinalReview,
        S::ShadowReady,
    ] {
        let next = transition(state, event, TransitionContext::default()).unwrap();
        assert_eq!(next.next_state, S::Planning);
        assert_eq!(next.effects, [Effect::DispatchPlanner]);
    }
    for state in [
        S::Building,
        S::Completed,
        S::Abandoned,
        S::TakenOver,
        S::Escalated,
        S::Blocked,
        S::Merging,
        S::ReadyToMerge,
    ] {
        assert!(transition(state, event, TransitionContext::default()).is_err());
    }
    let next = transition(
        S::Reviewing,
        event,
        TransitionContext {
            remediation_round: 3,
            max_remediation_rounds: 3,
            ..TransitionContext::default()
        },
    )
    .unwrap();
    assert_eq!(next.next_state, S::Escalated);
    assert_eq!(next.effects, [Effect::Escalate]);
}
