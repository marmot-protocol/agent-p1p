use std::collections::BTreeSet;
use std::num::{NonZeroU32, NonZeroU64};

use pip_core::{
    ActorId, IntakeBlocker, IntakeDecision, IntakePolicy, IssueObservation, PolicyRevision,
    evaluate_intake,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
struct Fixture {
    fixture_format: u32,
    base: Value,
    cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize)]
struct Case {
    name: String,
    set: serde_json::Map<String, Value>,
    expected_blockers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Input {
    policy: PolicyDto,
    observation: ObservationDto,
}

#[derive(Debug, Deserialize)]
struct PolicyDto {
    revision: u64,
    intake_enabled: bool,
    dispatch_enabled: bool,
    global_paused: bool,
    repository_paused: bool,
    required_label: String,
    trusted_actor_ids: Vec<u64>,
    repository_active_limit: u32,
    global_active_limit: u32,
}

#[derive(Debug, Deserialize)]
struct ObservationDto {
    open: bool,
    is_pull_request: bool,
    labels: Vec<String>,
    latest_label_actor_id: Option<u64>,
    excluded: bool,
    held: bool,
    already_owned: bool,
    repository_active_cases: u32,
    global_active_cases: u32,
}

fn set_path(root: &mut Value, path: &str, value: Value) {
    let (first, second) = path.split_once('.').expect("fixture paths have two parts");
    root[first][second] = value;
}

fn evaluate(value: Value) -> IntakeDecision {
    let input: Input = serde_json::from_value(value).unwrap();
    let policy = IntakePolicy {
        revision: PolicyRevision::new(NonZeroU64::new(input.policy.revision).unwrap()),
        intake_enabled: input.policy.intake_enabled,
        dispatch_enabled: input.policy.dispatch_enabled,
        global_paused: input.policy.global_paused,
        repository_paused: input.policy.repository_paused,
        required_label: input.policy.required_label,
        trusted_actor_ids: input
            .policy
            .trusted_actor_ids
            .into_iter()
            .map(|value| ActorId::new(NonZeroU64::new(value).unwrap()))
            .collect(),
        repository_active_limit: NonZeroU32::new(input.policy.repository_active_limit).unwrap(),
        global_active_limit: NonZeroU32::new(input.policy.global_active_limit).unwrap(),
    };
    let observation = IssueObservation {
        open: input.observation.open,
        is_pull_request: input.observation.is_pull_request,
        labels: input
            .observation
            .labels
            .into_iter()
            .collect::<BTreeSet<_>>(),
        latest_label_actor_id: input
            .observation
            .latest_label_actor_id
            .map(|value| ActorId::new(NonZeroU64::new(value).unwrap())),
        excluded: input.observation.excluded,
        held: input.observation.held,
        already_owned: input.observation.already_owned,
        repository_active_cases: input.observation.repository_active_cases,
        global_active_cases: input.observation.global_active_cases,
    };
    evaluate_intake(&policy, &observation)
}

#[test]
fn language_neutral_intake_policy_cases_pass() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../migration/target-v1/intake-cases.json"
    ))
    .unwrap();
    assert_eq!(fixture.fixture_format, 1);

    for case in fixture.cases {
        let mut input = fixture.base.clone();
        for (path, value) in case.set {
            set_path(&mut input, &path, value);
        }
        let observed = match evaluate(input) {
            IntakeDecision::Eligible => Vec::new(),
            IntakeDecision::Ineligible(blockers) => blockers
                .into_iter()
                .map(|blocker| blocker.to_string())
                .collect(),
        };
        assert_eq!(observed, case.expected_blockers, "{}", case.name);
    }
}

#[test]
fn every_intake_blocker_has_a_stable_wire_name() {
    let names = IntakeBlocker::ALL.map(|blocker| blocker.to_string());
    assert_eq!(names.len(), BTreeSet::from(names.clone()).len());
    assert!(names.iter().all(|name| !name.is_empty()));
}
