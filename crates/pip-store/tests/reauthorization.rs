use pip_store::{
    ApplyResult, EffectInput, EventInput, FaultPoint, NewCase, PolicyInput, Store, TransitionInput,
};
use serde_json::json;

fn fixture(path: &std::path::Path) -> (Store, TransitionInput) {
    let mut store = Store::open(path).unwrap();
    for revision in [1, 2] {
        store
            .record_policy(&PolicyInput {
                repository_id: 123,
                revision,
                accepted_at: 1,
                payload: json!({"revision":revision,"intake":{"label":"approved","trusted_actor_ids":[100]}}),
            })
            .unwrap();
    }
    store
        .create_case(&NewCase {
            case_key: "repo:123#45@1".into(),
            repository_id: 123,
            issue_number: 45,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 10,
            event: EventInput {
                event_id: "initial".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label_event_id":10}),
            },
            effects: vec![],
        })
        .unwrap();
    let mut input = TransitionInput {
        case_key: "repo:123#45@1".into(),
        expected_revision: 1,
        next_state: "ABANDONED".into(),
        remediation_round: 0,
        plan_version: 1,
        pr_number: None,
        head_sha: None,
        observed_at: 20,
        event: EventInput {
            event_id: "withdrawn".into(),
            event_type: "AUTHORIZATION_REMOVED".into(),
            payload: json!({"blockers":["LATEST_AUTHORIZATION_REMOVED"]}),
        },
        run: None,
        evidence: vec![],
        findings: vec![],
        effects: vec![],
    };
    store.apply_transition(&input, None).unwrap();
    input.expected_revision = 2;
    input.next_state = "PLANNING".into();
    input.observed_at = 30;
    input.event = EventInput {
        event_id: "reauthorized".into(),
        event_type: "ISSUE_REAUTHORIZED".into(),
        payload: json!({"label":"approved","label_actor_id":100,"label_event_id":30,"removed_label_event_id":20,"policy_revision":2}),
    };
    input.effects = vec![EffectInput {
        effect_id: "new-planner".into(),
        effect_type: "DISPATCH_PLANNER".into(),
        payload: json!({"case_key":input.case_key,"state_revision":3,"effect":"DISPATCH_PLANNER"}),
    }];
    (store, input)
}

#[test]
fn reauthorization_is_atomic_replayable_and_preserves_plan_versions() {
    for fault in [
        FaultPoint::AfterEvent,
        FaultPoint::AfterRun,
        FaultPoint::AfterEvidence,
        FaultPoint::AfterProjection,
        FaultPoint::AfterOutbox,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (mut store, input) = fixture(&directory.path().join("ledger.db"));
        let before = store.immutable_history_for_case(&input.case_key).unwrap();
        assert!(store.apply_transition(&input, Some(fault)).is_err());
        assert_eq!(
            store.immutable_history_for_case(&input.case_key).unwrap(),
            before
        );
        assert_eq!(
            store
                .case(&input.case_key)
                .unwrap()
                .unwrap()
                .policy_revision,
            1
        );
        assert_eq!(store.status(30).unwrap().outbox_total, 0);
        assert_eq!(
            store.apply_transition(&input, None).unwrap(),
            ApplyResult::Applied
        );
        assert_eq!(
            store.apply_transition(&input, None).unwrap(),
            ApplyResult::Replayed
        );
        let case = store.case(&input.case_key).unwrap().unwrap();
        assert_eq!(
            (case.state_revision, case.plan_version, case.policy_revision),
            (3, 1, 2)
        );
        assert_eq!(
            store.reconstruct_case(&input.case_key).unwrap().unwrap(),
            case
        );
        assert_eq!(store.case_created_at(&input.case_key).unwrap(), Some(10));
        assert_eq!(store.case_authorized_at(&input.case_key).unwrap(), Some(30));
    }
}

#[test]
fn reauthorization_cannot_change_accepted_plan_or_reuse_old_label_evidence() {
    for scenario in ["plan", "head", "old-label", "actor", "effect"] {
        let directory = tempfile::tempdir().unwrap();
        let (mut store, mut input) = fixture(&directory.path().join("ledger.db"));
        match scenario {
            "plan" => input.plan_version = 0,
            "head" => input.head_sha = Some("a".repeat(40)),
            "old-label" => input.event.payload["label_event_id"] = json!(10),
            "actor" => input.event.payload["label_actor_id"] = json!(999),
            "effect" => input.effects[0].effect_type = "DISPATCH_BUILDER".into(),
            _ => unreachable!(),
        }
        assert!(store.apply_transition(&input, None).is_err(), "{scenario}");
        assert_eq!(
            store.case(&input.case_key).unwrap().unwrap().state,
            "ABANDONED"
        );
    }
}

#[test]
fn historical_policy_lookup_is_exact_and_survives_reauthorization() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.db");
    let (mut store, input) = fixture(&path);
    store.apply_transition(&input, None).unwrap();
    drop(store);
    let store = Store::open_read_only(&path).unwrap();
    for (state_revision, policy_revision) in [(1, 1), (2, 1), (3, 2)] {
        assert_eq!(
            store
                .accepted_policy_at_case_revision(&input.case_key, state_revision)
                .unwrap(),
            store.accepted_policy(123, policy_revision).unwrap()
        );
    }
    for revision in [0, 4, 999] {
        assert!(
            store
                .accepted_policy_at_case_revision(&input.case_key, revision)
                .is_err()
        );
    }
    assert!(
        store
            .accepted_policy_at_case_revision("foreign-case", 1)
            .is_err()
    );
}
