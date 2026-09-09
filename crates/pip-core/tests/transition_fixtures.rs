use std::str::FromStr;

use pip_core::{
    CaseState, Effect, Event, MergeMode, TransitionContext, TransitionError, transition,
};
use proptest::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Fixture {
    fixture_format: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    #[serde(rename = "from")]
    from_state: String,
    event: String,
    remediation_round: Option<u32>,
    max_rounds: Option<u32>,
    merge_mode: Option<String>,
    #[serde(rename = "to")]
    to_state: String,
}

#[test]
fn curated_target_transition_oracle_passes() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../migration/target-v1/transitions.json"
    ))
    .expect("transition fixture must be valid JSON");

    assert_eq!(fixture.fixture_format, 1);
    for case in fixture.cases {
        let state = CaseState::from_str(&case.from_state).expect("known source state");
        let event = Event::from_str(&case.event).expect("known event");
        let expected = CaseState::from_str(&case.to_state).expect("known target state");
        let merge_mode = match case.merge_mode.as_deref().unwrap_or("shadow") {
            "shadow" => MergeMode::Shadow,
            "guarded" => MergeMode::Guarded,
            value => panic!("{}: unknown merge mode {value}", case.name),
        };
        let observed = transition(
            state,
            event,
            TransitionContext {
                remediation_round: case.remediation_round.unwrap_or(0),
                max_remediation_rounds: case.max_rounds.unwrap_or(3),
                merge_mode,
            },
        )
        .unwrap_or_else(|error| panic!("{}: {error}", case.name));

        assert_eq!(observed.next_state, expected, "{}", case.name);
    }
}

#[test]
fn terminal_states_reject_work_and_only_allow_merge_classification_correction() {
    for state in [
        CaseState::Completed,
        CaseState::Abandoned,
        CaseState::TakenOver,
    ] {
        for event in Event::ALL {
            if state == CaseState::TakenOver && event == Event::HumanMerged {
                let result = transition(state, event, TransitionContext::default()).unwrap();
                assert_eq!(result.next_state, CaseState::Completed);
                assert_eq!(result.effects, [Effect::RecordCompletion]);
                continue;
            }
            assert_eq!(
                transition(state, event, TransitionContext::default()),
                Err(TransitionError::TerminalState(state))
            );
        }
    }
}

#[test]
fn loop_bounds_must_be_positive() {
    assert_eq!(
        transition(
            CaseState::Reviewing,
            Event::RequestChanges,
            TransitionContext {
                remediation_round: 0,
                max_remediation_rounds: 0,
                merge_mode: MergeMode::Shadow,
            },
        ),
        Err(TransitionError::InvalidLoopBound)
    );
}

#[test]
fn an_external_operational_bound_escalates_every_automated_state() {
    for state in [
        CaseState::Planning,
        CaseState::ReadyToBuild,
        CaseState::Building,
        CaseState::WaitingCi,
        CaseState::Reviewing,
        CaseState::Remediating,
        CaseState::FinalReview,
        CaseState::ReadyToMerge,
        CaseState::Merging,
    ] {
        let decision = transition(
            state,
            Event::OperationalBoundReached,
            TransitionContext::default(),
        )
        .unwrap();
        assert_eq!(decision.next_state, CaseState::Escalated);
        assert_eq!(decision.effects, [Effect::Escalate]);
    }
}

#[test]
fn unknown_serialized_values_fail_closed() {
    assert!(CaseState::from_str("reviewing").is_err());
    assert!(Event::from_str("AUTO_APPROVE").is_err());
}

#[test]
fn accepted_work_emits_only_the_next_declared_effect() {
    let recorded = transition(
        CaseState::Planning,
        Event::PlanRecorded,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(recorded.next_state, CaseState::Planning);
    assert_eq!(recorded.effects, [Effect::PublishPlan]);

    let build = transition(
        CaseState::Planning,
        Event::Proceed,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(build.effects, [Effect::DispatchBuilder]);

    let review = transition(
        CaseState::ReadyToBuild,
        Event::BuilderDispatched,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(review.effects, []);

    let recorded = transition(
        CaseState::Building,
        Event::BuildRecorded,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(recorded.next_state, CaseState::Building);
    assert_eq!(recorded.effects, [Effect::PublishDraftPullRequest]);

    let final_review = transition(
        CaseState::Reviewing,
        Event::ReviewsApproved,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(final_review.effects, [Effect::PublishReviews]);

    let published = transition(
        CaseState::FinalReview,
        Event::ReviewsPublished,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(published.next_state, CaseState::FinalReview);
    assert_eq!(published.effects, [Effect::ObserveFinalPreflight]);

    let verified = transition(
        CaseState::FinalReview,
        Event::FinalPreflightAccepted,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(verified.next_state, CaseState::FinalReview);
    assert_eq!(verified.effects, [Effect::DispatchFinalReviewer]);
}

#[test]
fn remediation_builder_waits_until_both_reviews_are_published() {
    let remediation = transition(
        CaseState::Reviewing,
        Event::RequestChanges,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(remediation.next_state, CaseState::Remediating);
    assert_eq!(remediation.effects, [Effect::PublishReviews]);

    let published = transition(
        CaseState::Remediating,
        Event::ReviewsPublished,
        TransitionContext::default(),
    )
    .unwrap();
    assert_eq!(published.effects, [Effect::DispatchBuilder]);
}

#[test]
fn first_independent_review_is_recorded_without_advancing_or_dispatching() {
    let recorded = transition(
        CaseState::Reviewing,
        Event::ReviewRecorded,
        TransitionContext::default(),
    )
    .unwrap();

    assert_eq!(recorded.next_state, CaseState::Reviewing);
    assert!(recorded.effects.is_empty());
}

#[test]
fn default_shadow_policy_cannot_emit_a_merge_effect() {
    let ready = transition(
        CaseState::FinalReview,
        Event::Ready,
        TransitionContext::default(),
    )
    .unwrap();

    assert_eq!(ready.next_state, CaseState::ShadowReady);
    assert_eq!(ready.effects, [Effect::NotifyShadowReady]);
    assert!(!ready.effects.contains(&Effect::BeginMerge));
}

#[test]
fn guarded_merge_preparation_releases_only_the_deterministic_transaction() {
    let prepared = transition(
        CaseState::ReadyToMerge,
        Event::MergeStarted,
        TransitionContext {
            merge_mode: MergeMode::Guarded,
            ..TransitionContext::default()
        },
    )
    .unwrap();
    assert_eq!(prepared.next_state, CaseState::Merging);
    assert_eq!(prepared.effects, [Effect::ExecuteMerge]);
}

#[test]
fn authorization_removal_abandons_every_nonterminal_state_without_new_work() {
    for state in [
        CaseState::Planning,
        CaseState::WaitingHuman,
        CaseState::ReadyToBuild,
        CaseState::Building,
        CaseState::WaitingCi,
        CaseState::Reviewing,
        CaseState::Remediating,
        CaseState::FinalReview,
        CaseState::ShadowReady,
        CaseState::ReadyToMerge,
        CaseState::Merging,
        CaseState::Blocked,
        CaseState::Escalated,
    ] {
        let decision = transition(
            state,
            Event::AuthorizationRemoved,
            TransitionContext::default(),
        )
        .unwrap();
        assert_eq!(decision.next_state, CaseState::Abandoned);
        assert_eq!(decision.effects, [Effect::RecordAbandonment]);
    }
}

proptest! {
    #[test]
    fn review_remediation_escalates_exactly_at_the_policy_bound(
        remediation_round in any::<u32>(),
        max_rounds in 1_u32..=u32::MAX,
    ) {
        let observed = transition(
            CaseState::Reviewing,
            Event::RequestChanges,
            TransitionContext {
                remediation_round,
                max_remediation_rounds: max_rounds,
                merge_mode: MergeMode::Shadow,
            },
        ).unwrap();
        let expected = if remediation_round >= max_rounds {
            CaseState::Escalated
        } else {
            CaseState::Remediating
        };
        prop_assert_eq!(observed.next_state, expected);
    }

    #[test]
    fn transition_evaluation_is_deterministic(
        state_index in 0_usize..13,
        event_index in 0_usize..Event::ALL.len(),
        remediation_round in any::<u32>(),
        max_rounds in 1_u32..=u32::MAX,
    ) {
        let state = [
            CaseState::Planning,
            CaseState::WaitingHuman,
            CaseState::ReadyToBuild,
            CaseState::Building,
            CaseState::WaitingCi,
            CaseState::Reviewing,
            CaseState::Remediating,
            CaseState::FinalReview,
            CaseState::ShadowReady,
            CaseState::ReadyToMerge,
            CaseState::Merging,
            CaseState::Blocked,
            CaseState::Escalated,
        ][state_index];
        let input = TransitionContext {
            remediation_round,
            max_remediation_rounds: max_rounds,
            merge_mode: MergeMode::Shadow,
        };
        let first = transition(state, Event::ALL[event_index], input);
        let replay = transition(state, Event::ALL[event_index], input);
        prop_assert_eq!(first, replay);
    }
}
