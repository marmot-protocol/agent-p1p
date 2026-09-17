use pip_store::{ApplyResult, EventInput, NewCase, Store, TransitionInput};
use serde_json::json;

#[test]
fn only_explicit_unbounded_ready_follow_up_starts_a_new_window() {
    for scenario in [
        "fresh", "legacy", "bounded", "active", "terminal", "no-pr", "numeric",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let mut store = Store::open(&path).unwrap();
        store
            .create_case(&NewCase {
                case_key: "repo:42#9@3".into(),
                repository_id: 42,
                issue_number: 9,
                workflow_version: 3,
                policy_revision: 1,
                initial_state: "SHADOW_READY".into(),
                observed_at: 100,
                event: EventInput {
                    event_id: "seed".into(),
                    event_type: "SEED".into(),
                    payload: json!({}),
                },
                effects: vec![],
            })
            .unwrap();
        let mut transition = TransitionInput {
            case_key: "repo:42#9@3".into(),
            expected_revision: 1,
            next_state: if scenario == "active" {
                "REVIEWING"
            } else {
                "SHADOW_READY"
            }
            .into(),
            remediation_round: 2,
            plan_version: 1,
            pr_number: Some(77),
            head_sha: Some("b".repeat(40)),
            observed_at: 110,
            event: EventInput {
                event_id: "ready".into(),
                event_type: "READY".into(),
                payload: json!({}),
            },
            run: None,
            evidence: vec![],
            findings: vec![],
            effects: vec![],
        };
        store.apply_transition(&transition, None).unwrap();
        assert_eq!(
            store.case_work_started_at(&transition.case_key).unwrap(),
            Some(100)
        );
        transition.expected_revision = 2;
        transition.next_state = if scenario == "terminal" {
            "ESCALATED"
        } else {
            "PLANNING"
        }
        .into();
        transition.observed_at = 1000;
        transition.event = EventInput {
            event_id: "feedback".into(),
            event_type: "HUMAN_FEEDBACK_RECEIVED".into(),
            payload: json!({"bounded":false,"fresh_work_window":true}),
        };
        match scenario {
            "legacy" => transition.event.payload = json!({"bounded":false}),
            "bounded" => transition.event.payload["bounded"] = json!(true),
            "numeric" => transition.event.payload["fresh_work_window"] = json!(1),
            "no-pr" => {
                transition.pr_number = None;
                transition.head_sha = None;
            }
            _ => {}
        }
        store.apply_transition(&transition, None).unwrap();
        assert_eq!(
            store.apply_transition(&transition, None).unwrap(),
            ApplyResult::Replayed
        );
        drop(store);
        let store = Store::open_read_only(&path).unwrap();
        assert_eq!(
            store.case_created_at(&transition.case_key).unwrap(),
            Some(100)
        );
        assert_eq!(
            store.case_authorized_at(&transition.case_key).unwrap(),
            Some(100)
        );
        assert_eq!(
            store.case_work_started_at(&transition.case_key).unwrap(),
            Some(if scenario == "fresh" { 1000 } else { 100 }),
            "{scenario}"
        );
        assert_eq!(store.case_work_started_at("missing").unwrap(), None);
        assert_eq!(
            store
                .immutable_history_for_case(&transition.case_key)
                .unwrap()
                .events
                .len(),
            3
        );
    }
}
