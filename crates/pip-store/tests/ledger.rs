use std::path::Path;

use pip_store::{
    ApplyResult, DirectAttemptStatus, EffectInput, EventInput, EvidenceInput, FaultPoint,
    FindingInput, NewCase, PolicyInput, RunInput, Store, StoreError, TaskProjectionInput,
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
        evidence: vec![EvidenceInput {
            evidence_id: "evidence-plan-comment-1".into(),
            kind: "GITHUB_COMMENT".into(),
            source: "github".into(),
            payload: json!({"comment_id": 10001}),
        }],
        findings: vec![FindingInput {
            finding_id: "GENERAL-R1-001".into(),
            origin_role: "reviewer-general".into(),
            reviewed_head_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            payload: json!({"status": "OPEN"}),
        }],
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
    assert_eq!(store.schema_version().unwrap(), 4);
    assert!(store.foreign_keys_enabled().unwrap());
    assert_eq!(store.journal_mode().unwrap(), "wal");
}

#[test]
fn direct_attempts_preserve_terminal_results_and_audit_history() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claimed = store
        .claim_effect("direct-worker-1", 100, 30)
        .unwrap()
        .unwrap();

    let attempt_id = store
        .begin_direct_attempt(&claimed, "planner-task-1", 101)
        .unwrap();
    assert_eq!(attempt_id, 1);
    store
        .complete_direct_attempt(
            attempt_id,
            "direct-worker-1",
            102,
            &json!({"schema_version": 1, "outcome": "PROCEED"}),
        )
        .unwrap();

    let attempt = store
        .completed_direct_attempt("effect-planner-1")
        .unwrap()
        .unwrap();
    assert_eq!(
        store.direct_attempt(attempt_id).unwrap(),
        Some(attempt.clone())
    );
    assert_eq!(attempt.attempt_id, 1);
    assert_eq!(attempt.task_id, "planner-task-1");
    assert_eq!(attempt.status, DirectAttemptStatus::Complete);
    assert_eq!(attempt.started_at, 101);
    assert_eq!(attempt.completed_at, Some(102));
    assert_eq!(
        attempt.result,
        Some(json!({"schema_version": 1, "outcome": "PROCEED"}))
    );
    assert_eq!(attempt.result_sha256.unwrap().len(), 64);
    assert_eq!(store.status(102).unwrap().direct_attempts_complete, 1);

    assert!(matches!(
        store.complete_direct_attempt(
            attempt_id,
            "direct-worker-1",
            103,
            &json!({"schema_version": 1, "outcome": "STOP"}),
        ),
        Err(StoreError::LeaseLost(_))
    ));
    let connection = Connection::open(directory.path().join("ledger.db")).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE direct_attempts SET error = 'tampered' WHERE attempt_id = 1",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute("DELETE FROM direct_attempts WHERE attempt_id = 1", [])
            .is_err()
    );
}

#[test]
fn a_new_lease_closes_an_abandoned_attempt_before_starting_another() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let first = store
        .claim_effect("direct-worker-1", 100, 10)
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .begin_direct_attempt(&first, "planner-task-1", 100)
            .unwrap(),
        1
    );

    let second = store
        .claim_effect("direct-worker-2", 111, 10)
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .begin_direct_attempt(&second, "planner-task-1", 111)
            .unwrap(),
        2
    );
    let status = store.status(111).unwrap();
    assert_eq!(status.direct_attempts_running, 1);
    assert_eq!(status.direct_attempts_failed, 1);
    assert_eq!(status.direct_attempts_complete, 0);
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
    assert_eq!(case.repository_id, 984_321);
    assert_eq!(case.issue_number, 1240);
    assert_eq!(case.workflow_version, 1);
    assert_eq!(case.state, "PLANNING");
    assert_eq!(case.state_revision, 1);
    assert_eq!(store.event_count().unwrap(), 1);
    assert_eq!(store.outbox_count().unwrap(), 1);
}

#[test]
fn immutable_case_history_returns_ordered_payloads_with_stored_digests() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    store.apply_transition(&transition(), None).unwrap();

    let history = store
        .immutable_history_for_case("repo:984321#1240@1")
        .unwrap();
    assert_eq!(history.events.len(), 2);
    assert_eq!(history.events[0].event_id, "event-intake-1");
    assert_eq!(history.events[1].event_id, "event-plan-1");
    assert_eq!(history.events[1].payload, json!({"plan_version": 1}));
    assert_eq!(history.events[1].payload_sha256.len(), 64);
    assert_eq!(history.runs.len(), 1);
    assert_eq!(history.runs[0].run_id, "run-plan-1");
    assert_eq!(history.runs[0].payload_sha256.len(), 64);
    assert_eq!(history.evidence.len(), 1);
    assert_eq!(history.evidence[0].evidence_id, "evidence-plan-comment-1");
    assert_eq!(history.evidence[0].payload_sha256.len(), 64);
    assert_eq!(history.findings.len(), 1);
    assert_eq!(history.findings[0].finding_id, "GENERAL-R1-001");
    assert_eq!(history.findings[0].payload_sha256.len(), 64);
}

#[test]
fn operator_status_separates_pending_leased_and_delivered_work() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    store
        .claim_effect("controller-1", 100, 30)
        .unwrap()
        .unwrap();

    let status = store.status(110).unwrap();
    assert_eq!(status.schema_version, 4);
    assert_eq!(status.cases.len(), 1);
    assert_eq!(status.cases[0].case_key, "repo:984321#1240@1");
    assert_eq!(status.events, 1);
    assert_eq!(status.outbox_total, 1);
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 1);
    assert_eq!(status.outbox_delivered, 0);
    assert_eq!(status.outbox_superseded, 0);

    store
        .acknowledge_effect("effect-planner-1", "controller-1", 111)
        .unwrap();
    let status = store.status(112).unwrap();
    assert_eq!(status.outbox_pending, 0);
    assert_eq!(status.outbox_leased, 0);
    assert_eq!(status.outbox_delivered, 1);
    assert_eq!(status.outbox_superseded, 0);
}

#[test]
fn a_new_transition_atomically_supersedes_older_undelivered_effects() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();

    store.apply_transition(&transition(), None).unwrap();
    let status = store.status(1_787_000_601).unwrap();
    assert_eq!(status.outbox_total, 2);
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_delivered, 0);
    assert_eq!(status.outbox_superseded, 1);
    let claimed = store
        .claim_effect("controller", 1_787_000_602, 30)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.effect_id, "effect-builder-1");

    let (_directory, mut rollback) = open();
    rollback.create_case(&new_case()).unwrap();
    assert!(
        rollback
            .apply_transition(&transition(), Some(FaultPoint::AfterOutbox))
            .is_err()
    );
    let status = rollback.status(1_787_000_601).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_superseded, 0);
    assert_eq!(
        rollback
            .claim_effect("controller", 1_787_000_602, 30)
            .unwrap()
            .unwrap()
            .effect_id,
        "effect-planner-1"
    );
}

#[test]
fn crash_injection_rolls_back_every_transition_boundary() {
    for fault in [
        FaultPoint::AfterEvent,
        FaultPoint::AfterRun,
        FaultPoint::AfterEvidence,
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
        assert_eq!(store.evidence_count().unwrap(), 0);
        assert_eq!(store.finding_count().unwrap(), 0);
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
    assert_eq!(store.evidence_count().unwrap(), 1);
    assert_eq!(store.finding_count().unwrap(), 1);
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
    store.apply_transition(&transition(), None).unwrap();
    drop(store);
    let connection = Connection::open(directory.path().join("ledger.db")).unwrap();
    assert!(
        connection
            .execute("UPDATE events SET event_type = 'tampered'", [])
            .is_err()
    );
    assert!(connection.execute("DELETE FROM events", []).is_err());
    assert!(
        connection
            .execute("UPDATE evidence SET source = 'tampered'", [])
            .is_err()
    );
    assert!(connection.execute("DELETE FROM findings", []).is_err());
}

#[test]
fn versioned_policy_is_immutable_idempotent_evidence() {
    let (_directory, mut store) = open();
    let policy = PolicyInput {
        repository_id: 984_321,
        revision: 1,
        accepted_at: 1_787_000_000,
        payload: json!({"merge_mode": "shadow", "intake_label": "pip-ok"}),
    };
    assert_eq!(store.record_policy(&policy).unwrap(), ApplyResult::Applied);
    assert_eq!(store.record_policy(&policy).unwrap(), ApplyResult::Replayed);
    let mut conflicting = policy.clone();
    conflicting.payload["merge_mode"] = json!("guarded");
    assert!(matches!(
        store.record_policy(&conflicting),
        Err(StoreError::IdempotencyConflict { ref id }) if id == "policy:984321:1"
    ));
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
fn dispatch_claim_skips_other_durable_effect_types_without_leasing_them() {
    let (_directory, mut store) = open();
    let mut case = new_case();
    case.effects = vec![
        EffectInput {
            effect_id: "effect-observe-ci".into(),
            effect_type: "OBSERVE_CI".into(),
            payload: json!({"case_key": case.case_key}),
        },
        EffectInput {
            effect_id: "effect-dispatch-planner".into(),
            effect_type: "DISPATCH_PLANNER".into(),
            payload: json!({"case_key": case.case_key}),
        },
    ];
    store.create_case(&case).unwrap();

    let claimed = store
        .claim_effect_matching(
            "dispatcher",
            100,
            30,
            &[
                "DISPATCH_PLANNER",
                "DISPATCH_BUILDER",
                "DISPATCH_REVIEWERS",
                "DISPATCH_FINAL_REVIEWER",
            ],
        )
        .unwrap()
        .unwrap();
    assert_eq!(claimed.effect_id, "effect-dispatch-planner");
    store
        .acknowledge_effect(&claimed.effect_id, "dispatcher", 101)
        .unwrap();
    assert_eq!(
        store
            .claim_effect("observer", 102, 30)
            .unwrap()
            .unwrap()
            .effect_id,
        "effect-observe-ci"
    );
}

#[test]
fn effect_owner_can_release_a_live_lease_for_immediate_retry() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claimed = store.claim_effect("observer-a", 100, 30).unwrap().unwrap();

    assert!(
        store
            .release_effect(&claimed.effect_id, "wrong-owner")
            .is_err()
    );
    store
        .release_effect(&claimed.effect_id, "observer-a")
        .unwrap();

    let retried = store.claim_effect("observer-b", 100, 30).unwrap().unwrap();
    assert_eq!(retried.effect_id, claimed.effect_id);
    assert_eq!(retried.lease_owner, "observer-b");
}

#[test]
fn external_effect_evidence_and_delivery_commit_atomically_and_replay() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claimed = store.claim_effect("consumer", 100, 30).unwrap().unwrap();
    let evidence = EvidenceInput {
        evidence_id: format!("evidence-effect-{}", claimed.effect_id),
        kind: "GITHUB_EFFECT".into(),
        source: "github".into(),
        payload: json!({"result":"created","external_id":9001}),
    };

    assert_eq!(
        store
            .complete_effect_evidence(&claimed.effect_id, "consumer", 101, &evidence)
            .unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(store.evidence_count().unwrap(), 1);
    assert_eq!(store.status(101).unwrap().outbox_delivered, 1);
    assert_eq!(
        store
            .complete_effect_evidence(&claimed.effect_id, "retry", 102, &evidence)
            .unwrap(),
        ApplyResult::Replayed
    );

    let conflicting = EvidenceInput {
        payload: json!({"result":"different"}),
        ..evidence
    };
    assert!(matches!(
        store.complete_effect_evidence(&claimed.effect_id, "retry", 102, &conflicting),
        Err(StoreError::IdempotencyConflict { .. })
    ));
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
fn backup_restores_only_to_a_new_destination() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let backup = directory.path().join("ledger.backup.db");
    store.backup_to(&backup).unwrap();

    let restored = directory.path().join("ledger.restored.db");
    Store::restore_backup_to_new(&backup, &restored).unwrap();
    assert!(
        Store::open_read_only(&restored)
            .unwrap()
            .case("repo:984321#1240@1")
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        Store::restore_backup_to_new(&backup, &restored),
        Err(StoreError::DestinationExists(_))
    ));
}

#[test]
fn newer_schema_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("future.db");
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::UnsupportedSchema(99))
    ));
}

#[test]
fn schema_one_upgrades_forward_without_losing_existing_projections() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v1.db");
    let mut store = Store::open(&path).unwrap();
    store.create_case(&new_case()).unwrap();
    let effect = store.claim_effect("projector", 100, 30).unwrap().unwrap();
    let projection = TaskProjectionInput {
        projection_id: "legacy-projection".into(),
        effect_id: effect.effect_id,
        board: "pip-mdk".into(),
        task_id: "legacy-task".into(),
        desired: json!({"status": "blocked"}),
        observed: json!({"id": "legacy-task"}),
    };
    store
        .complete_task_projection(&projection, "projector", 101, None)
        .unwrap();
    drop(store);

    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE direct_attempts;
             DROP INDEX outbox_dispatchable;
             ALTER TABLE outbox DROP COLUMN superseded_by_event_id;
             ALTER TABLE outbox DROP COLUMN superseded_at;
             ALTER TABLE task_projections RENAME TO task_projections_v2;
             CREATE TABLE task_projections (
                 projection_id TEXT PRIMARY KEY,
                 case_key TEXT NOT NULL REFERENCES cases(case_key),
                 effect_id TEXT NOT NULL UNIQUE REFERENCES outbox(effect_id),
                 board TEXT NOT NULL,
                 task_id TEXT,
                 desired_json TEXT NOT NULL,
                 observed_json TEXT,
                 reconciled_at INTEGER
             ) STRICT;
             INSERT INTO task_projections SELECT * FROM task_projections_v2;
             DROP TABLE task_projections_v2;
             DELETE FROM schema_migrations WHERE version >= 2;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);

    let upgraded = Store::open(&path).unwrap();
    assert_eq!(upgraded.schema_version().unwrap(), 4);
    assert_eq!(
        upgraded
            .task_projection("legacy-projection")
            .unwrap()
            .unwrap(),
        projection
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

#[test]
fn task_projection_and_outbox_delivery_commit_together_and_replay() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claimed = store.claim_effect("projector-a", 100, 30).unwrap().unwrap();
    let projection = TaskProjectionInput {
        projection_id: "repo:984321#1240@1:planner:1".into(),
        effect_id: claimed.effect_id.clone(),
        board: "pip-mdk".into(),
        task_id: "task-1".into(),
        desired: json!({"status": "blocked", "assignee": "planner"}),
        observed: json!({"id": "task-1", "status": "blocked"}),
    };
    assert!(matches!(
        store.complete_task_projection(
            &projection,
            "projector-a",
            110,
            Some(FaultPoint::AfterProjection),
        ),
        Err(StoreError::InjectedFault(FaultPoint::AfterProjection))
    ));
    assert_eq!(store.task_projection_count().unwrap(), 0);
    assert!(
        store
            .claim_effect("projector-b", 120, 30)
            .unwrap()
            .is_none()
    );

    let reclaimed = store.claim_effect("projector-b", 131, 30).unwrap().unwrap();
    assert_eq!(reclaimed.effect_id, claimed.effect_id);
    assert_eq!(
        store
            .complete_task_projection(&projection, "projector-b", 132, None)
            .unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store
            .complete_task_projection(&projection, "projector-b", 133, None)
            .unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(store.task_projection_count().unwrap(), 1);
    assert_eq!(
        store
            .task_projection(&projection.projection_id)
            .unwrap()
            .unwrap(),
        projection
    );
    assert!(
        store
            .claim_effect("projector-c", 200, 30)
            .unwrap()
            .is_none()
    );
}

#[test]
fn one_dispatch_effect_atomically_records_gate_and_worker_projections() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claimed = store.claim_effect("projector-a", 100, 30).unwrap().unwrap();
    let projections = [
        TaskProjectionInput {
            projection_id: "repo:984321#1240@1:planner:gate".into(),
            effect_id: claimed.effect_id.clone(),
            board: "pip-mdk".into(),
            task_id: "gate-1".into(),
            desired: json!({"kind": "gate", "status": "blocked"}),
            observed: json!({"id": "gate-1", "status": "blocked"}),
        },
        TaskProjectionInput {
            projection_id: "repo:984321#1240@1:planner:worker".into(),
            effect_id: claimed.effect_id.clone(),
            board: "pip-mdk".into(),
            task_id: "worker-1".into(),
            desired: json!({"kind": "worker", "status": "blocked"}),
            observed: json!({"id": "worker-1", "status": "blocked"}),
        },
    ];

    assert_eq!(
        store
            .complete_task_projections(&projections, "projector-a", 110, None)
            .unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(store.task_projection_count().unwrap(), 2);
    assert_eq!(
        store
            .complete_task_projections(&projections, "projector-a", 111, None)
            .unwrap(),
        ApplyResult::Replayed
    );
}

#[test]
fn direct_worker_enqueue_and_dispatch_ack_are_atomic_and_retry_safe() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claimed = store.claim_effect("projector-a", 100, 30).unwrap().unwrap();
    let jobs = [EffectInput {
        effect_id: "effect-planner-1:direct:builder".into(),
        effect_type: "RUN_DIRECT_WORKER".into(),
        payload: json!({
            "schema_version": 1,
            "source_effect_id": claimed.effect_id,
            "task_id": "direct-builder-1",
            "role": "builder"
        }),
    }];

    assert!(matches!(
        store.complete_dispatch_outputs(
            &claimed.effect_id,
            &[],
            &jobs,
            "projector-a",
            110,
            Some(FaultPoint::AfterOutbox),
        ),
        Err(StoreError::InjectedFault(FaultPoint::AfterOutbox))
    ));
    let status = store.status(110).unwrap();
    assert_eq!(status.outbox_total, 1);
    assert_eq!(status.outbox_delivered, 0);

    let reclaimed = store.claim_effect("projector-b", 131, 30).unwrap().unwrap();
    assert_eq!(reclaimed.effect_id, claimed.effect_id);
    assert_eq!(
        store
            .complete_dispatch_outputs(&claimed.effect_id, &[], &jobs, "projector-b", 132, None,)
            .unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store
            .complete_dispatch_outputs(&claimed.effect_id, &[], &jobs, "projector-c", 133, None,)
            .unwrap(),
        ApplyResult::Replayed
    );
    let job = store
        .claim_effect_matching("direct-worker", 134, 30, &["RUN_DIRECT_WORKER"])
        .unwrap()
        .unwrap();
    assert_eq!(job.effect_id, "effect-planner-1:direct:builder");
    assert_eq!(job.payload["task_id"], "direct-builder-1");
}

#[test]
fn consecutive_builder_dispatch_and_result_transitions_commit_atomically() {
    let (_directory, mut store) = open();
    let mut case = new_case();
    case.initial_state = "READY_TO_BUILD".into();
    case.effects = vec![EffectInput {
        effect_id: "effect-run-direct-builder".into(),
        effect_type: "RUN_DIRECT_WORKER".into(),
        payload: json!({"task_id": "builder-1"}),
    }];
    store.create_case(&case).unwrap();
    let transitions = [
        TransitionInput {
            case_key: case.case_key.clone(),
            expected_revision: 1,
            next_state: "BUILDING".into(),
            remediation_round: 0,
            plan_version: 1,
            pr_number: None,
            head_sha: None,
            observed_at: 100,
            event: EventInput {
                event_id: "event-builder-dispatched".into(),
                event_type: "BUILDER_DISPATCHED".into(),
                payload: json!({"task_id": "builder-1"}),
            },
            run: None,
            evidence: Vec::new(),
            findings: Vec::new(),
            effects: Vec::new(),
        },
        TransitionInput {
            case_key: case.case_key.clone(),
            expected_revision: 2,
            next_state: "BUILDING".into(),
            remediation_round: 0,
            plan_version: 1,
            pr_number: None,
            head_sha: None,
            observed_at: 101,
            event: EventInput {
                event_id: "event-build-recorded".into(),
                event_type: "BUILD_RECORDED".into(),
                payload: json!({"head_sha": "b".repeat(40)}),
            },
            run: Some(RunInput {
                run_id: "run-builder-1".into(),
                task_id: "builder-1".into(),
                role: "builder".into(),
                payload: json!({"outcome": "REVIEW_READY"}),
            }),
            evidence: Vec::new(),
            findings: Vec::new(),
            effects: vec![EffectInput {
                effect_id: "effect-publish-draft".into(),
                effect_type: "PUBLISH_DRAFT_PULL_REQUEST".into(),
                payload: json!({"task_id": "builder-1"}),
            }],
        },
    ];

    assert!(matches!(
        store.apply_transition_batch(&transitions, Some(FaultPoint::BetweenTransitions)),
        Err(StoreError::InjectedFault(FaultPoint::BetweenTransitions))
    ));
    assert_eq!(
        store.case(&case.case_key).unwrap().unwrap().state_revision,
        1
    );
    assert_eq!(store.event_count().unwrap(), 1);
    assert_eq!(store.run_count().unwrap(), 0);
    assert_eq!(store.status(101).unwrap().outbox_pending, 1);

    assert_eq!(
        store.apply_transition_batch(&transitions, None).unwrap(),
        ApplyResult::Applied
    );
    let stored = store.case(&case.case_key).unwrap().unwrap();
    assert_eq!(stored.state, "BUILDING");
    assert_eq!(stored.state_revision, 3);
    assert_eq!(store.event_count().unwrap(), 3);
    assert_eq!(store.run_count().unwrap(), 1);
    let status = store.status(101).unwrap();
    assert_eq!(status.outbox_superseded, 1);
    assert_eq!(status.outbox_pending, 1);
}
