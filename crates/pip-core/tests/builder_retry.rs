use pip_core::{CaseState, Effect, Event, MergeMode, TransitionContext, transition};

#[test]
fn review_recovery_returns_to_ci_not_approval_or_build() {
    let context = TransitionContext {
        merge_mode: MergeMode::Shadow,
        remediation_round: 0,
        max_remediation_rounds: 3,
    };
    let decision = transition(CaseState::Escalated, Event::ReviewRetryAuthorized, context).unwrap();
    assert_eq!(decision.next_state, CaseState::WaitingCi);
    assert_eq!(decision.effects, vec![Effect::ObserveCi]);
    for state in [
        CaseState::Planning,
        CaseState::Building,
        CaseState::Reviewing,
        CaseState::FinalReview,
        CaseState::Completed,
        CaseState::Abandoned,
    ] {
        assert!(transition(state, Event::ReviewRetryAuthorized, context).is_err());
    }
}

#[test]
fn operator_retry_can_redispatch_a_ready_or_escalated_builder() {
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
    assert_eq!(
        transition(CaseState::Escalated, Event::BuilderRetryAuthorized, context).unwrap(),
        decision
    );
    for state in [
        CaseState::Planning,
        CaseState::Building,
        CaseState::Reviewing,
        CaseState::Completed,
    ] {
        assert!(transition(state, Event::BuilderRetryAuthorized, context).is_err());
    }
}
