use pip_store::{EffectInput, EventInput, NewCase, Store, TransitionInput};
use serde_json::json;

const CASE: &str = "repo:984321#1240@1";

fn open() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "BUILDING".into(),
            observed_at: 100,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![EffectInput {
                effect_id: "build-a".into(),
                effect_type: "RUN_DIRECT_WORKER".into(),
                payload: json!({}),
            }],
        })
        .unwrap();
    (directory, store)
}

fn attempt(store: &mut Store, owner: &str, now: u64) -> u64 {
    let claimed = store.claim_effect(owner, now, 600).unwrap().unwrap();
    store.begin_direct_attempt(&claimed, "task", now).unwrap()
}

fn next_stage(store: &mut Store, revision: u64, effect_id: &str, now: u64) {
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: revision,
                next_state: "BUILDING".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: now,
                event: EventInput {
                    event_id: format!("event-{revision}"),
                    event_type: "CI_FAILED".into(),
                    payload: json!({}),
                },
                run: None,
                evidence: vec![],
                findings: vec![],
                effects: vec![EffectInput {
                    effect_id: effect_id.into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: json!({}),
                }],
            },
            None,
        )
        .unwrap();
}

#[test]
fn work_failures_cool_down_and_count_only_against_the_current_stage() {
    let (_directory, mut store) = open();
    let first = attempt(&mut store, "worker", 1000);
    assert_eq!(
        store
            .fail_direct_attempt_with_cooldown(first, "worker", 1010, "invalid result", 120)
            .unwrap(),
        1130
    );
    assert_eq!(store.direct_stage_failure_count(CASE).unwrap(), 1);
    // The failed stage is not reclaimable until its cooldown passes.
    assert!(store.claim_effect("worker", 1100, 600).unwrap().is_none());
    let second = attempt(&mut store, "worker", 1131);
    store
        .fail_direct_attempt_with_cooldown(second, "worker", 1140, "invalid result", 120)
        .unwrap();
    assert_eq!(store.direct_stage_failure_count(CASE).unwrap(), 2);

    // A new stage starts with a fresh budget; history keeps every failure.
    next_stage(&mut store, 1, "build-b", 1200);
    assert_eq!(store.direct_stage_failure_count(CASE).unwrap(), 0);
    assert_eq!(store.failed_direct_attempt_count_for_case(CASE).unwrap(), 2);
}

#[test]
fn outages_never_spend_the_stage_budget_and_report_when_they_began() {
    let (_directory, mut store) = open();
    assert_eq!(store.direct_stage_outage(CASE).unwrap(), None);
    let first = attempt(&mut store, "worker", 1000);
    store
        .record_direct_unavailability(first, "worker", 1005, "rate limit exceeded")
        .unwrap();
    let second = attempt(&mut store, "worker", 1100);
    store
        .record_direct_unavailability_with_cause(
            second,
            "worker",
            1105,
            "Cannot use this model: x",
            "MODEL_UNAVAILABLE",
        )
        .unwrap();
    assert_eq!(store.direct_stage_failure_count(CASE).unwrap(), 0);
    let outage = store.direct_stage_outage(CASE).unwrap().unwrap();
    assert_eq!(outage.effect_id, "build-a");
    assert_eq!(outage.since, 1005);
    assert_eq!(outage.cause, "MODEL_UNAVAILABLE");
    assert_eq!(outage.error, "Cannot use this model: x");

    // A real work attempt ends the outage streak.
    let third = attempt(&mut store, "worker", 1400);
    store
        .fail_direct_attempt_with_cooldown(third, "worker", 1410, "invalid result", 120)
        .unwrap();
    assert_eq!(store.direct_stage_outage(CASE).unwrap(), None);
    assert_eq!(store.direct_stage_failure_count(CASE).unwrap(), 1);
}

#[test]
fn an_expired_worker_lease_is_an_outage_not_a_work_failure() {
    let (_directory, mut store) = open();
    let claimed = store.claim_effect("worker", 1000, 60).unwrap().unwrap();
    store.begin_direct_attempt(&claimed, "task", 1000).unwrap();
    // The worker died; another claim after lease expiry closes the attempt.
    let second = attempt(&mut store, "worker", 1100);
    assert_eq!(store.direct_stage_failure_count(CASE).unwrap(), 0);
    // The replacement is running, so there is no outage right now.
    assert_eq!(store.direct_stage_outage(CASE).unwrap(), None);
    // If it also cannot run, the outage dates from the expired lease.
    store
        .record_direct_unavailability(second, "worker", 1105, "network error")
        .unwrap();
    let outage = store.direct_stage_outage(CASE).unwrap().unwrap();
    assert_eq!(outage.since, 1100);
    assert_eq!(outage.cause, "PROVIDER_UNAVAILABLE");
}

#[test]
fn a_worker_that_starts_after_an_outage_ends_it() {
    let (_directory, mut store) = open();
    let first = attempt(&mut store, "worker", 1000);
    store
        .record_direct_unavailability(first, "worker", 1005, "status 503")
        .unwrap();
    assert_eq!(
        store.direct_stage_outage(CASE).unwrap().unwrap().since,
        1005
    );
    // The provider recovered and a long build is now running.
    attempt(&mut store, "worker", 1100);
    assert_eq!(store.direct_stage_outage(CASE).unwrap(), None);
}
