use pip_core::{CaseState, Effect, Event, TransitionContext, transition};

const PARKED: [CaseState; 3] = [
    CaseState::Escalated,
    CaseState::Blocked,
    CaseState::WaitingHuman,
];

fn resume(state: CaseState, target: Option<CaseState>) -> Result<(CaseState, Vec<Effect>), String> {
    transition(
        state,
        Event::HumanResumed,
        TransitionContext {
            resume_target: target,
            // Resuming never depends on the remaining remediation budget.
            remediation_round: 10,
            max_remediation_rounds: 10,
            ..TransitionContext::default()
        },
    )
    .map(|decision| (decision.next_state, decision.effects))
    .map_err(|error| error.to_string())
}

#[test]
fn a_parked_case_resumes_at_the_chosen_stage_and_acknowledges_the_human() {
    for state in PARKED {
        for (target, dispatch) in [
            (CaseState::Planning, Effect::DispatchPlanner),
            (CaseState::ReadyToBuild, Effect::DispatchBuilder),
            (CaseState::Remediating, Effect::DispatchBuilder),
            (CaseState::WaitingCi, Effect::ObserveCi),
        ] {
            assert_eq!(
                resume(state, Some(target)).unwrap(),
                (target, vec![dispatch, Effect::AcknowledgeResume]),
                "{state} -> {target}"
            );
        }
    }
}

#[test]
fn a_resume_needs_a_restartable_target_and_a_parked_case() {
    for state in PARKED {
        for target in [
            None,
            Some(CaseState::FinalReview),
            Some(CaseState::Completed),
        ] {
            assert!(resume(state, target).is_err(), "{state} -> {target:?}");
        }
    }
    for state in [
        CaseState::Planning,
        CaseState::Reviewing,
        CaseState::ShadowReady,
        CaseState::Completed,
        CaseState::TakenOver,
    ] {
        assert!(resume(state, Some(CaseState::Planning)).is_err(), "{state}");
    }
}

#[test]
fn only_a_parked_case_can_be_abandoned_by_command() {
    for state in PARKED {
        let decision =
            transition(state, Event::HumanAbandoned, TransitionContext::default()).unwrap();
        assert_eq!(decision.next_state, CaseState::Abandoned);
        assert_eq!(decision.effects, [Effect::RecordAbandonment]);
    }
    for state in [CaseState::Building, CaseState::Completed] {
        assert!(transition(state, Event::HumanAbandoned, TransitionContext::default()).is_err());
    }
}

#[test]
fn resume_events_round_trip_by_name() {
    for (name, event) in [
        ("HUMAN_RESUMED", Event::HumanResumed),
        ("HUMAN_ABANDONED", Event::HumanAbandoned),
    ] {
        assert_eq!(name.parse::<Event>().unwrap(), event);
        assert_eq!(event.to_string(), name);
        assert!(Event::ALL.contains(&event));
    }
}
