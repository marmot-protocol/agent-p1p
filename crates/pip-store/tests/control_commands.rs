use pip_store::{
    ApplyResult, ControlCommandInput, ControlCommandStatus, EventInput, NewCase, PolicyInput,
    Store, TransitionInput,
};
use serde_json::json;

const CASE: &str = "repo:984321#1240@1";

fn open() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    for revision in [1, 2] {
        store
            .record_policy(&PolicyInput {
                repository_id: 984_321,
                revision,
                accepted_at: 1,
                payload: json!({"revision": revision}),
            })
            .unwrap();
    }
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "WAITING_CI".into(),
            observed_at: 100,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    (directory, store)
}

fn transition(
    store: &mut Store,
    revision: u64,
    event: &str,
    next: &str,
    payload: serde_json::Value,
) {
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: revision,
                next_state: next.into(),
                remediation_round: 10,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 100 + revision,
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

fn command(comment_id: u64, received_at: u64) -> ControlCommandInput {
    ControlCommandInput {
        comment_id,
        repository_id: 984_321,
        thread_number: 1240,
        case_key: Some(CASE.into()),
        actor_id: 202_880,
        command: "RESUME".into(),
        guidance: "Try again; the provider outage is over.".into(),
        received_at,
    }
}

#[test]
fn control_commands_are_recorded_once_and_resolved_explicitly() {
    let (_directory, mut store) = open();
    assert_eq!(
        store.record_control_command(&command(7, 200)).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store.record_control_command(&command(7, 205)).unwrap(),
        ApplyResult::Replayed
    );
    let pending = store.pending_control_commands(984_321, Some(CASE)).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].comment_id, 7);
    assert_eq!(pending[0].command, "RESUME");
    assert_eq!(pending[0].status, ControlCommandStatus::Pending);

    store
        .resolve_control_command(
            7,
            ControlCommandStatus::Applied,
            "resumed at WAITING_CI",
            210,
        )
        .unwrap();
    assert!(
        store
            .pending_control_commands(984_321, None)
            .unwrap()
            .is_empty()
    );
    // A resolved command cannot be resolved differently later.
    assert!(
        store
            .resolve_control_command(7, ControlCommandStatus::Ignored, "late", 220)
            .is_err()
    );
}

#[test]
fn the_parking_event_explains_where_a_case_stopped() {
    let (_directory, mut store) = open();
    assert!(store.parking_event(CASE).unwrap().is_none());
    transition(
        &mut store,
        1,
        "CI_FAILED",
        "ESCALATED",
        json!({"blockers": ["Required CI"]}),
    );
    let park = store.parking_event(CASE).unwrap().unwrap();
    assert_eq!(park.event_type, "CI_FAILED");
    assert_eq!(park.previous_state, "WAITING_CI");
    assert_eq!(park.state_revision, 2);
    assert_eq!(park.observed_at, 101);
    assert_eq!(park.payload["blockers"][0], "Required CI");
}

#[test]
fn a_resume_rebinds_policy_and_grants_rounds_through_the_ledger() {
    let (_directory, mut store) = open();
    transition(&mut store, 1, "CI_FAILED", "ESCALATED", json!({}));
    assert_eq!(store.granted_remediation_rounds(CASE).unwrap(), 0);
    transition(
        &mut store,
        2,
        "HUMAN_RESUMED",
        "REMEDIATING",
        json!({"comment_id": 7, "policy_revision": 2, "granted_rounds": 3}),
    );
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(case.state, "REMEDIATING");
    assert_eq!(case.policy_revision, 2);
    assert_eq!(store.granted_remediation_rounds(CASE).unwrap(), 3);
    assert!(store.parking_event(CASE).unwrap().is_none());
}

#[test]
fn a_resume_cannot_rebind_to_an_unknown_or_older_policy() {
    for revision in [0, 9] {
        let (_directory, mut store) = open();
        transition(&mut store, 1, "CI_FAILED", "ESCALATED", json!({}));
        let result = store.apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: 2,
                next_state: "WAITING_CI".into(),
                remediation_round: 10,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 300,
                event: EventInput {
                    event_id: "resume".into(),
                    event_type: "HUMAN_RESUMED".into(),
                    payload: json!({"policy_revision": revision, "granted_rounds": 0}),
                },
                run: None,
                evidence: vec![],
                findings: vec![],
                effects: vec![],
            },
            None,
        );
        assert!(result.is_err(), "{revision}");
    }
}

#[test]
fn activity_pending_work_and_finding_windows_follow_the_ledger() {
    let (_directory, mut store) = open();
    assert_eq!(store.last_case_activity(CASE).unwrap(), 100);
    assert_eq!(store.finding_window_start(CASE).unwrap(), 0);
    transition(&mut store, 1, "CI_FAILED", "ESCALATED", json!({}));
    assert_eq!(store.last_case_activity(CASE).unwrap(), 101);
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: 2,
                next_state: "WAITING_CI".into(),
                remediation_round: 10,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 500,
                event: EventInput {
                    event_id: "resume".into(),
                    event_type: "HUMAN_RESUMED".into(),
                    payload: json!({"policy_revision": 1, "granted_rounds": 0}),
                },
                run: None,
                evidence: vec![],
                findings: vec![],
                effects: vec![pip_store::EffectInput {
                    effect_id: "observe-ci".into(),
                    effect_type: "OBSERVE_CI".into(),
                    payload: json!({}),
                }],
            },
            None,
        )
        .unwrap();
    assert_eq!(store.last_case_activity(CASE).unwrap(), 500);
    assert_eq!(store.finding_window_start(CASE).unwrap(), 500);
    assert_eq!(store.pending_effect_types(CASE).unwrap(), ["OBSERVE_CI"]);
    // A resume starts a fresh age window.
    assert_eq!(store.case_work_started_at(CASE).unwrap(), Some(500));
}
