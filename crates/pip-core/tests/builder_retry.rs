use pip_core::{CaseState, Effect, Event, MergeMode, TransitionContext, transition};

#[test]
fn operator_retry_only_redispatches_a_ready_builder() {
    let context = TransitionContext {
        merge_mode: MergeMode::Shadow,
        remediation_round: 0,
        max_remediation_rounds: 3,
    };
    let decision = transition(
        CaseState::ReadyToBuild,
        Event::BuilderRetryAuthorized,
        context,
    )
    .unwrap();
    assert_eq!(decision.next_state, CaseState::ReadyToBuild);
    assert_eq!(decision.effects, vec![Effect::DispatchBuilder]);
    for state in [
        CaseState::Planning,
        CaseState::Building,
        CaseState::Reviewing,
        CaseState::Escalated,
        CaseState::Completed,
    ] {
        assert!(transition(state, Event::BuilderRetryAuthorized, context).is_err());
    }
}
