use pip_core::{CaseState, Effect, Event, MergeMode, TransitionContext, transition};

#[test]
fn native_review_recovery_preserves_exhausted_remediation_budget() {
    let event: Event = "NATIVE_REVIEW_RETRY_AUTHORIZED".parse().unwrap();
    let context = TransitionContext {
        remediation_round: 10,
        max_remediation_rounds: 10,
        ..TransitionContext::default()
    };
    let decision = transition(CaseState::Escalated, event, context).unwrap();
    assert_eq!(decision.next_state, CaseState::WaitingCi);
    assert_eq!(decision.effects, [Effect::ObserveCi]);
    for state in [
        CaseState::Reviewing,
        CaseState::TakenOver,
        CaseState::Completed,
        CaseState::Abandoned,
    ] {
        assert!(transition(state, event, context).is_err());
    }
}

#[test]
fn planner_recovery_only_reopens_escalated_work_for_a_fresh_plan() {
    let event: Event = "PLANNER_RETRY_AUTHORIZED".parse().unwrap();
    let context = TransitionContext::default();
    let decision = transition(CaseState::Escalated, event, context).unwrap();
    assert_eq!(decision.next_state, CaseState::Planning);
    assert_eq!(decision.effects, [Effect::DispatchPlanner]);
    for state in [
        CaseState::Planning,
        CaseState::TakenOver,
        CaseState::Abandoned,
        CaseState::Completed,
        CaseState::ReadyToBuild,
        CaseState::ShadowReady,
    ] {
        assert!(transition(state, event, context).is_err());
    }
}

#[test]
fn publication_recovery_only_releases_publication_not_another_agent_attempt() {
    let event: Event = "PUBLICATION_RETRY_AUTHORIZED".parse().unwrap();
    let context = TransitionContext {
        remediation_round: 1,
        ..TransitionContext::default()
    };
    for state in [
        CaseState::WaitingCi,
        CaseState::Reviewing,
        CaseState::FinalReview,
    ] {
        let decision = transition(state, event, context).unwrap();
        assert_eq!(decision.next_state, CaseState::Building);
        assert_eq!(decision.effects, [Effect::PublishDraftPullRequest]);
    }
    for state in [
        CaseState::Planning,
        CaseState::ReadyToBuild,
        CaseState::Building,
        CaseState::Remediating,
        CaseState::Escalated,
        CaseState::ShadowReady,
        CaseState::Completed,
        CaseState::Abandoned,
        CaseState::TakenOver,
    ] {
        assert!(transition(state, event, context).is_err());
    }
}

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
