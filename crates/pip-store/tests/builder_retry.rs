use pip_store::{
    ApplyResult, EffectInput, EventInput, FaultPoint, NewCase, PolicyInput, RunInput, Store,
    TransitionInput,
};
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
fn retry_preserves_history_and_allows_only_one_additional_failure() {
    let (_dir, mut store, input) = fixture();
    let history = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(
        store.apply_transition(&input, None).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store.apply_transition(&input, None).unwrap(),
        ApplyResult::Replayed
    );
    let after = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(after.runs, history.runs);
    assert_eq!(store.failed_direct_attempt_count_for_case(CASE).unwrap(), 3);
    assert_eq!(store.effective_provider_failure_limit(CASE, 3).unwrap(), 4);
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(
        (case.state_revision, case.policy_revision, case.plan_version),
        (3, 7, 1)
    );
    let effect = store.claim_effect("worker", 150, 5).unwrap().unwrap();
    assert_eq!(effect.effect_id, "retry-dispatch");
    assert_eq!(store.status(150).unwrap().outbox_superseded, 1);
}

#[test]
fn retry_rejects_invalid_scope_or_live_work_without_mutating_history() {
    for scenario in [
        "wrong-count",
        "non-root",
        "blank-reason",
        "wrong-state",
        "wrong-plan",
        "wrong-effect",
        "lease",
        "running",
        "expired",
        "extra-effects",
    ] {
        let (_dir, mut store, mut input) = fixture();
        match scenario {
            "wrong-count" => input.event.payload["failed_attempts"] = json!(2),
            "non-root" => input.event.payload["operator_uid"] = json!(1000),
            "blank-reason" => input.event.payload["reason"] = json!(" "),
            "wrong-state" => input.next_state = "BUILDING".into(),
            "wrong-plan" => input.plan_version = 2,
            "wrong-effect" => input.event.payload["effect_id"] = json!("foreign"),
            "expired" => input.observed_at = 86500,
            "extra-effects" => input.effects.push(input.effects[0].clone()),
            "lease" | "running" => {
                let effect = store.claim_effect("worker", 135, 30).unwrap().unwrap();
                if scenario == "running" {
                    store
                        .begin_direct_attempt(&effect, "old-task", 135)
                        .unwrap();
                }
            }
            _ => unreachable!(),
        }
        let before = store.status(140).unwrap();
        assert!(store.apply_transition(&input, None).is_err(), "{scenario}");
        assert_eq!(store.status(140).unwrap(), before);
    }
}

#[test]
fn retry_fault_rolls_back_authorization_and_new_dispatch_together() {
    for fault in [
        FaultPoint::AfterEvent,
        FaultPoint::AfterProjection,
        FaultPoint::AfterOutbox,
    ] {
        let (_dir, mut store, input) = fixture();
        let before = store.status(140).unwrap();
        assert!(store.apply_transition(&input, Some(fault)).is_err());
        assert_eq!(store.status(140).unwrap(), before);
        assert_eq!(store.effective_provider_failure_limit(CASE, 3).unwrap(), 3);
    }
}
