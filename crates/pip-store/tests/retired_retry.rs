use pip_store::{EffectInput, EventInput, NewCase, PolicyInput, RunInput, Store, TransitionInput};
use serde_json::json;

const CASE: &str = "repo:42#9@1";

fn fixture() -> (tempfile::TempDir, Store, TransitionInput) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("ledger.db")).unwrap();
    store
        .record_policy(&PolicyInput {
            repository_id: 42,
            revision: 7,
            accepted_at: 100,
            payload: json!({"max_provider_failures":3,"max_case_elapsed_seconds":86400}),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 42,
            issue_number: 9,
            workflow_version: 1,
            policy_revision: 7,
            initial_state: "PLANNING".into(),
            observed_at: 100,
            event: EventInput {
                event_id: "intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: 1,
                next_state: "READY_TO_BUILD".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 101,
                event: EventInput {
                    event_id: "plan".into(),
                    event_type: "PROCEED".into(),
                    payload: json!({}),
                },
                run: Some(RunInput {
                    run_id: "accepted-plan".into(),
                    task_id: "planner".into(),
                    role: "planner".into(),
                    payload: json!({"plan_version":1}),
                }),
                evidence: vec![],
                findings: vec![],
                effects: vec![EffectInput {
                    effect_id: "old-builder".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: json!({"role":"builder","task_id":"old-task"}),
                }],
            },
            None,
        )
        .unwrap();
    for now in [110, 120, 130] {
        let effect = store.claim_effect("worker", now, 5).unwrap().unwrap();
        let attempt = store
            .begin_direct_attempt(&effect, "old-task", now)
            .unwrap();
        store
            .fail_direct_attempt(attempt, "worker", now + 1, "startup failed")
            .unwrap();
        store.release_effect(&effect.effect_id, "worker").unwrap();
    }
    let input = TransitionInput {
        case_key: CASE.into(),
        expected_revision: 2,
        next_state: "READY_TO_BUILD".into(),
        remediation_round: 0,
        plan_version: 1,
        pr_number: None,
        head_sha: None,
        observed_at: 140,
        event: EventInput {
            event_id: "retry-operator-1".into(),
            event_type: "BUILDER_RETRY_AUTHORIZED".into(),
            payload: json!({"schema_version":1,"effect_id":"old-builder","failed_attempts":3,"base_failure_limit":3,"operator_uid":0,"reason":"Repaired worker checkout"}),
        },
        run: None,
        evidence: vec![],
        findings: vec![],
        effects: vec![EffectInput {
            effect_id: "retry-dispatch".into(),
            effect_type: "DISPATCH_BUILDER".into(),
            payload: json!({"case_key":CASE,"state_revision":3,"effect":"DISPATCH_BUILDER","remediation_round":0,"plan_version":1,"pr_number":null,"head_sha":null}),
        }],
    };
    (dir, store, input)
}

#[test]
fn retired_lifetime_retry_events_are_rejected_without_mutating_history() {
    for event_type in [
        "BUILDER_RETRY_AUTHORIZED",
        "REVIEW_RETRY_AUTHORIZED",
        "PLANNER_RETRY_AUTHORIZED",
    ] {
        let (_dir, mut store, mut input) = fixture();
        input.event.event_type = event_type.into();
        let history = store.immutable_history_for_case(CASE).unwrap();
        assert!(
            store.apply_transition(&input, None).is_err(),
            "{event_type}"
        );
        assert_eq!(store.immutable_history_for_case(CASE).unwrap(), history);
    }
}
