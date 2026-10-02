use pip_core::{CaseState, Effect, Event, TransitionContext, transition};

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
fn retired_lifetime_retry_events_are_not_part_of_the_workflow() {
    for name in [
        "BUILDER_RETRY_AUTHORIZED",
        "REVIEW_RETRY_AUTHORIZED",
        "PLANNER_RETRY_AUTHORIZED",
    ] {
        assert!(name.parse::<Event>().is_err(), "{name}");
    }
}
