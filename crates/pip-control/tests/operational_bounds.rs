use pip_control::{
    OperationalBound, OperationalBoundsCycle, enforce_operational_bounds, load_repository_policy,
};
use pip_store::{EffectInput, EventInput, FindingInput, NewCase, Store, TransitionInput};
use serde_json::json;

#[test]
fn elapsed_time_escalates_at_the_exact_policy_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    let mut policy = policy();
    policy.max_case_elapsed_seconds = 60;
    create_case(&mut store, 100);

    assert_eq!(
        enforce_operational_bounds(&mut store, &policy, 159).unwrap(),
        OperationalBoundsCycle::Idle
    );
    assert_eq!(
        enforce_operational_bounds(&mut store, &policy, 160).unwrap(),
        OperationalBoundsCycle::Escalated {
            case_key: "repo:1055628515#42@2".into(),
            bound: OperationalBound::ElapsedTime,
            observed: 60,
            limit: 60,
        }
    );
    assert_eq!(
        store.case("repo:1055628515#42@2").unwrap().unwrap().state,
        "ESCALATED"
    );
}

#[test]
fn fresh_human_follow_up_does_not_reset_provider_failures() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("ledger.db")).unwrap();
    let mut policy = policy();
    policy.max_case_elapsed_seconds = 60;
    policy.max_provider_failures = 1;
    create_case(&mut store, 100);
    let effect = store.claim_effect("worker", 110, 30).unwrap().unwrap();
    let attempt = store.begin_direct_attempt(&effect, "failed", 110).unwrap();
    store
        .fail_direct_attempt(attempt, "worker", 111, "provider unavailable")
        .unwrap();
    store.release_effect(&effect.effect_id, "worker").unwrap();
    for (revision, state, at, event, payload) in [
        (1, "SHADOW_READY", 120, "READY", json!({})),
        (
            2,
            "PLANNING",
            1000,
            "HUMAN_FEEDBACK_RECEIVED",
            json!({"bounded":false,"fresh_work_window":true}),
        ),
    ] {
        store
            .apply_transition(
                &TransitionInput {
                    case_key: "repo:1055628515#42@2".into(),
                    expected_revision: revision,
                    next_state: state.into(),
                    remediation_round: 2,
                    plan_version: 1,
                    pr_number: Some(77),
                    head_sha: Some("b".repeat(40)),
                    observed_at: at,
                    event: EventInput {
                        event_id: format!("event-{revision}"),
                        event_type: event.into(),
                        payload,
                    },
                    run: None,
                    evidence: vec![],
                    findings: vec![],
                    effects: vec![],
                },
                None,
            )
            .unwrap();
    }
    assert!(matches!(
        enforce_operational_bounds(&mut store, &policy, 1001).unwrap(),
        OperationalBoundsCycle::Escalated {
            bound: OperationalBound::ProviderFailures,
            observed: 1,
            limit: 1,
            ..
        }
    ));
    let case = store.case("repo:1055628515#42@2").unwrap().unwrap();
    assert_eq!(case.remediation_round, 2);
    assert_eq!(
        store
            .failed_direct_attempt_count_for_case(&case.case_key)
            .unwrap(),
        1
    );
}

#[test]
fn repeated_finding_fingerprints_and_provider_failures_are_bounded() {
    for scenario in ["finding", "provider"] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        let mut policy = policy();
        policy.max_repeated_finding_fingerprint = 2;
        policy.max_provider_failures = 2;
        create_case(&mut store, 100);

        if scenario == "finding" {
            for revision in 1..=2 {
                store
                    .apply_transition(
                        &TransitionInput {
                            case_key: "repo:1055628515#42@2".into(),
                            expected_revision: revision,
                            next_state: "PLANNING".into(),
                            remediation_round: 0,
                            plan_version: 0,
                            pr_number: None,
                            head_sha: None,
                            observed_at: 100 + revision,
                            event: EventInput {
                                event_id: format!("finding-event-{revision}"),
                                event_type: "PLAN_RECORDED".into(),
                                payload: json!({}),
                            },
                            run: None,
                            evidence: vec![],
                            findings: vec![FindingInput {
                                finding_id: format!("F-{revision}"),
                                origin_role: "reviewer-general".into(),
                                reviewed_head_sha: "a".repeat(40),
                                payload: json!({
                                    "id": format!("F-{revision}"),
                                    "defect": "same defect",
                                    "consequence": "same consequence",
                                    "corrective_direction": "same correction",
                                    "summary": "wording may change"
                                }),
                            }],
                            effects: vec![],
                        },
                        None,
                    )
                    .unwrap();
            }
        } else {
            for attempt in 0..2 {
                let claimed = store
                    .claim_effect("worker", 110 + attempt, 30)
                    .unwrap()
                    .unwrap();
                let attempt_id = store
                    .begin_direct_attempt(&claimed, &format!("task-{attempt}"), 110 + attempt)
                    .unwrap();
                store
                    .fail_direct_attempt(attempt_id, "worker", 111 + attempt, "provider failed")
                    .unwrap();
                store.release_effect(&claimed.effect_id, "worker").unwrap();
            }
        }

        let result = enforce_operational_bounds(&mut store, &policy, 200).unwrap();
        assert!(matches!(
            result,
            OperationalBoundsCycle::Escalated {
                bound: OperationalBound::RepeatedFindingFingerprint
                    | OperationalBound::ProviderFailures,
                observed: 2,
                limit: 2,
                ..
            }
        ));
    }
}

fn policy() -> pip_control::RepositoryPolicy {
    load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap()
}

fn create_case(store: &mut Store, observed_at: u64) {
    store
        .create_case(&NewCase {
            case_key: "repo:1055628515#42@2".into(),
            repository_id: 1_055_628_515,
            issue_number: 42,
            workflow_version: 2,
            policy_revision: policy().revision,
            initial_state: "PLANNING".into(),
            observed_at,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![EffectInput {
                effect_id: "effect-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({}),
            }],
        })
        .unwrap();
}
