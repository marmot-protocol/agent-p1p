use std::path::Path;

use pip_store::{
    ApplyResult, EffectInput, EventInput, FaultPoint, NewCase, RunInput, Store, StoreError,
    TransitionInput,
};
use rusqlite::Connection;
use serde_json::json;
use tempfile::TempDir;

fn open() -> (TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("ledger.db")).unwrap();
    (directory, store)
}

fn new_case() -> NewCase {
    NewCase {
        case_key: "repo:984321#1240@1".into(),
        repository_id: 984_321,
        issue_number: 1240,
        workflow_version: 1,
        policy_revision: 1,
        initial_state: "PLANNING".into(),
        observed_at: 1_787_000_000,
        event: EventInput {
            event_id: "event-intake-1".into(),
            event_type: "ISSUE_AUTHORIZED".into(),
            payload: json!({"actor_id": 1001, "label": "pip-ok"}),
        },
        effects: vec![EffectInput {
            effect_id: "effect-planner-1".into(),
            effect_type: "DISPATCH_PLANNER".into(),
            payload: json!({"case_key": "repo:984321#1240@1"}),
        }],
    }
}

fn transition() -> TransitionInput {
    TransitionInput {
        case_key: "repo:984321#1240@1".into(),
        expected_revision: 1,
        next_state: "READY_TO_BUILD".into(),
        remediation_round: 0,
        plan_version: 1,
        pr_number: Some(77),
        head_sha: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
        observed_at: 1_787_000_600,
        event: EventInput {
            event_id: "event-plan-1".into(),
            event_type: "PLANNER_PROCEED".into(),
            payload: json!({"plan_version": 1}),
        },
        run: Some(RunInput {
            run_id: "run-plan-1".into(),
            task_id: "planner-1".into(),
            role: "planner".into(),
            payload: json!({"outcome": "PROCEED"}),
        }),
        effects: vec![EffectInput {
            effect_id: "effect-builder-1".into(),
            effect_type: "DISPATCH_BUILDER".into(),
            payload: json!({"plan_version": 1}),
        }],
    }
}

#[test]
fn migration_creates_hardened_authoritative_schema() {
    let (_directory, store) = open();
    assert_eq!(store.schema_version().unwrap(), 1);
    assert!(store.foreign_keys_enabled().unwrap());
    assert_eq!(store.journal_mode().unwrap(), "wal");
}

#[test]
fn case_event_projection_and_outbox_are_one_transaction() {
    let (_directory, mut store) = open();
    assert_eq!(
        store.create_case(&new_case()).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store.create_case(&new_case()).unwrap(),
        ApplyResult::Replayed
    );

    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.state, "PLANNING");
    assert_eq!(case.state_revision, 1);
    assert_eq!(store.event_count().unwrap(), 1);
    assert_eq!(store.outbox_count().unwrap(), 1);
}

#[test]
fn crash_injection_rolls_back_every_transition_boundary() {
    for fault in [
        FaultPoint::AfterEvent,
        FaultPoint::AfterRun,
        FaultPoint::AfterProjection,
        FaultPoint::AfterOutbox,
    ] {
        let (_directory, mut store) = open();
        store.create_case(&new_case()).unwrap();
        assert!(matches!(
            store.apply_transition(&transition(), Some(fault)),
            Err(StoreError::InjectedFault(observed)) if observed == fault
        ));
        let case = store.case("repo:984321#1240@1").unwrap().unwrap();
        assert_eq!((case.state.as_str(), case.state_revision), ("PLANNING", 1));
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(store.run_count().unwrap(), 0);
        assert_eq!(store.outbox_count().unwrap(), 1);
    }
}

#[test]
fn accepted_transition_replays_without_duplicate_history_or_effects() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    assert_eq!(
        store.apply_transition(&transition(), None).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store.apply_transition(&transition(), None).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(store.event_count().unwrap(), 2);
    assert_eq!(store.run_count().unwrap(), 1);
    assert_eq!(store.outbox_count().unwrap(), 2);

    let projected = store.case("repo:984321#1240@1").unwrap().unwrap();
    let rebuilt = store
        .reconstruct_case("repo:984321#1240@1")
        .unwrap()
        .unwrap();
    assert_eq!(projected.state, rebuilt.state);
    assert_eq!(projected.state_revision, rebuilt.state_revision);
    assert_eq!(projected.policy_revision, rebuilt.policy_revision);
    assert_eq!(projected.plan_version, rebuilt.plan_version);
    assert_eq!(projected.pr_number, rebuilt.pr_number);
    assert_eq!(projected.head_sha, rebuilt.head_sha);
}

#[test]
fn immutable_history_detects_and_repairs_projection_corruption() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    store.apply_transition(&transition(), None).unwrap();
    drop(store);

    let path = directory.path().join("ledger.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE cases SET state = 'BLOCKED', state_revision = 99, plan_version = 99",
            [],
        )
        .unwrap();
    drop(connection);

    let mut store = Store::open(&path).unwrap();
    assert!(
        !store
            .projection_matches_history("repo:984321#1240@1")
            .unwrap()
    );
    store.repair_case_projection("repo:984321#1240@1").unwrap();
    assert!(
        store
            .projection_matches_history("repo:984321#1240@1")
            .unwrap()
    );
    let repaired = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(
        (repaired.state.as_str(), repaired.state_revision),
        ("READY_TO_BUILD", 2)
    );
    assert_eq!(repaired.plan_version, 1);
}

#[test]
fn same_id_with_different_payload_is_a_conflict() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let mut conflicting = transition();
    store.apply_transition(&conflicting, None).unwrap();
    conflicting.event.payload = json!({"plan_version": 2});
    assert!(matches!(
        store.apply_transition(&conflicting, None),
        Err(StoreError::IdempotencyConflict { ref id }) if id == "event-plan-1"
    ));
}

#[test]
fn immutable_history_rejects_direct_update_and_delete() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    drop(store);
    let connection = Connection::open(directory.path().join("ledger.db")).unwrap();
    assert!(
        connection
            .execute("UPDATE events SET event_type = 'tampered'", [])
            .is_err()
    );
    assert!(connection.execute("DELETE FROM events", []).is_err());
}

#[test]
fn outbox_leases_are_exclusive_recoverable_and_acknowledged() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claimed = store.claim_effect("worker-a", 100, 30).unwrap().unwrap();
    assert_eq!(claimed.effect_id, "effect-planner-1");
    assert!(store.claim_effect("worker-b", 110, 30).unwrap().is_none());
    let reclaimed = store.claim_effect("worker-b", 131, 30).unwrap().unwrap();
    assert_eq!(reclaimed.effect_id, claimed.effect_id);
    assert!(
        store
            .acknowledge_effect(&claimed.effect_id, "worker-a", 132)
            .is_err()
    );
    store
        .acknowledge_effect(&reclaimed.effect_id, "worker-b", 132)
        .unwrap();
    assert!(store.claim_effect("worker-c", 200, 30).unwrap().is_none());
}

#[test]
fn online_backup_is_a_complete_reopenable_ledger() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    store.apply_transition(&transition(), None).unwrap();
    let backup = directory.path().join("ledger.backup.db");
    store.backup_to(&backup).unwrap();

    let restored = Store::open(&backup).unwrap();
    let case = restored.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(
        (case.state.as_str(), case.state_revision),
        ("READY_TO_BUILD", 2)
    );
}

#[test]
fn database_path_is_never_implicitly_created_by_read_only_open() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.db");
    assert!(Store::open_read_only(&missing).is_err());
    assert!(!Path::new(&missing).exists());
}

#[test]
fn read_only_worker_handle_cannot_mutate_the_ledger() {
    let (directory, mut writer) = open();
    writer.create_case(&new_case()).unwrap();
    drop(writer);

    let mut reader = Store::open_read_only(directory.path().join("ledger.db")).unwrap();
    assert!(matches!(
        reader.create_case(&new_case()),
        Err(StoreError::ReadOnly)
    ));
}
