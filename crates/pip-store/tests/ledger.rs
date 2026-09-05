use std::path::Path;

use pip_store::{
    ApplyResult, CreateReservation, DirectAttemptStatus, DispatchIntent, DispatchTransport,
    EffectInput, EventInput, EvidenceInput, FaultPoint, FindingInput, NewCase, PolicyInput,
    ReviewObservationInput, RunInput, Store, StoreError, TaskProjectionInput, TransitionInput,
    WebhookDeliveryInput, WorkspaceRetirementInput, WorkspaceRetirementOutcome,
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
    assert_eq!(store.schema_version().unwrap(), 8);
    assert!(store.foreign_keys_enabled().unwrap());
    assert_eq!(store.journal_mode().unwrap(), "wal");
}

fn dispatch_intents() -> Vec<DispatchIntent> {
    vec![DispatchIntent {
        intent_id: "planner:worker".into(),
        transport: DispatchTransport::Hermes,
        desired: json!({"model": "configured-model", "body": {"evidence": "frozen"}}),
    }]
}

#[test]
fn dispatch_manifest_is_frozen_before_external_creation_and_survives_restart() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claim = store.claim_effect("controller", 100, 30).unwrap().unwrap();
    let intents = dispatch_intents();
    assert_eq!(
        store
            .freeze_dispatch_intents(&claim, &intents, 101)
            .unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store
            .freeze_dispatch_intents(&claim, &intents, 102)
            .unwrap(),
        ApplyResult::Replayed
    );
    let mut changed = intents.clone();
    changed[0].desired["model"] = json!("another-model");
    assert!(matches!(
        store.freeze_dispatch_intents(&claim, &changed, 103),
        Err(StoreError::IdempotencyConflict { .. })
    ));
    drop(store);
    let store = Store::open(directory.path().join("ledger.db")).unwrap();
    assert_eq!(
        store.dispatch_intents(&claim.effect_id).unwrap(),
        Some(intents)
    );
    assert_eq!(store.status(104).unwrap().task_projections, 0);
    assert_eq!(store.status(104).unwrap().outbox_delivered, 0);
    assert_eq!(store.status(104).unwrap().dispatch_batches, 1);
    assert_eq!(store.status(104).unwrap().dispatch_create_attempts, 0);
}

#[test]
fn dispatch_manifest_freezes_fanout_membership_and_rejects_invalid_inputs_atomically() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claim = store.claim_effect("controller", 100, 30).unwrap().unwrap();
    let one = dispatch_intents();
    for invalid in [
        vec![],
        vec![one[0].clone(), one[0].clone()],
        vec![DispatchIntent {
            desired: json!(null),
            ..one[0].clone()
        }],
    ] {
        assert!(
            store
                .freeze_dispatch_intents(&claim, &invalid, 101)
                .is_err()
        );
        assert!(store.dispatch_intents(&claim.effect_id).unwrap().is_none());
    }
    let mut two = one.clone();
    two.push(DispatchIntent {
        intent_id: "second:worker".into(),
        ..one[0].clone()
    });
    store.freeze_dispatch_intents(&claim, &two, 101).unwrap();
    assert!(matches!(
        store.freeze_dispatch_intents(&claim, &one, 102),
        Err(StoreError::IdempotencyConflict { .. })
    ));
    assert_eq!(
        store
            .reserve_dispatch_create(&claim, &two[0].intent_id, 102)
            .unwrap(),
        CreateReservation::Granted
    );
    assert_eq!(
        store
            .reserve_dispatch_create(&claim, &two[1].intent_id, 102)
            .unwrap(),
        CreateReservation::Granted
    );
}

#[test]
fn a_create_reservation_is_never_reissued_after_a_crash_or_lease_expiry() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claim = store.claim_effect("controller", 100, 30).unwrap().unwrap();
    let intents = dispatch_intents();
    store
        .freeze_dispatch_intents(&claim, &intents, 101)
        .unwrap();
    assert_eq!(
        store
            .reserve_dispatch_create(&claim, &intents[0].intent_id, 102)
            .unwrap(),
        CreateReservation::Granted
    );
    assert_eq!(
        store
            .reserve_dispatch_create(&claim, &intents[0].intent_id, 103)
            .unwrap(),
        CreateReservation::Uncertain
    );
    drop(store);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    // Even reusing the same owner does not let the old claim cross the new lease.
    let replacement = store.claim_effect("controller", 131, 30).unwrap().unwrap();
    assert!(matches!(
        store.reserve_dispatch_create(&claim, &intents[0].intent_id, 132),
        Err(StoreError::LeaseLost(_))
    ));
    assert_eq!(
        store
            .reserve_dispatch_create(&replacement, &intents[0].intent_id, 132)
            .unwrap(),
        CreateReservation::Uncertain
    );
}

#[test]
fn concurrent_create_reservations_grant_exactly_one_creator() {
    let (directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claim = store.claim_effect("controller", 100, 30).unwrap().unwrap();
    let intents = dispatch_intents();
    store
        .freeze_dispatch_intents(&claim, &intents, 101)
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let path = directory.path().join("ledger.db");
            let barrier = barrier.clone();
            let claim = claim.clone();
            std::thread::spawn(move || {
                let mut store = Store::open(path).unwrap();
                barrier.wait();
                store
                    .reserve_dispatch_create(&claim, "planner:worker", 102)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == CreateReservation::Granted)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == CreateReservation::Uncertain)
            .count(),
        1
    );
}

#[test]
fn a_projection_identity_cannot_receive_a_second_create_via_another_effect() {
    let (_directory, mut store) = open();
    let mut case = new_case();
    case.effects.push(EffectInput {
        effect_id: "effect-planner-2".into(),
        ..case.effects[0].clone()
    });
    store.create_case(&case).unwrap();
    let first = store
        .claim_effect("controller-1", 100, 30)
        .unwrap()
        .unwrap();
    let second = store
        .claim_effect("controller-2", 100, 30)
        .unwrap()
        .unwrap();
    let intents = dispatch_intents();
    store
        .freeze_dispatch_intents(&first, &intents, 101)
        .unwrap();
    store
        .freeze_dispatch_intents(&second, &intents, 101)
        .unwrap();
    assert_eq!(
        store
            .reserve_dispatch_create(&first, "planner:worker", 102)
            .unwrap(),
        CreateReservation::Granted
    );
    assert_eq!(
        store
            .reserve_dispatch_create(&second, "planner:worker", 102)
            .unwrap(),
        CreateReservation::Uncertain
    );
    assert_eq!(store.status(103).unwrap().dispatch_create_attempts, 1);
}

#[test]
fn stale_revoked_and_delivered_dispatch_cannot_freeze_or_reserve() {
    for mode in ["expired", "revoked", "delivered"] {
        let (_directory, mut store) = open();
        store.create_case(&new_case()).unwrap();
        let claim = store.claim_effect("controller", 100, 30).unwrap().unwrap();
        let intents = dispatch_intents();
        store
            .freeze_dispatch_intents(&claim, &intents, 101)
            .unwrap();
        let now = match mode {
            "expired" => 131,
            "revoked" => {
                let mut revoke = transition();
                revoke.next_state = "ABANDONED".into();
                revoke.event.event_type = "AUTHORIZATION_REMOVED".into();
                revoke.effects.clear();
                store.apply_transition(&revoke, None).unwrap();
                102
            }
            _ => {
                store
                    .acknowledge_effect(&claim.effect_id, "controller", 102)
                    .unwrap();
                103
            }
        };
        assert!(matches!(
            store.freeze_dispatch_intents(&claim, &intents, now),
            Err(StoreError::LeaseLost(_))
        ));
        assert!(matches!(
            store.reserve_dispatch_create(&claim, &intents[0].intent_id, now),
            Err(StoreError::LeaseLost(_))
        ));
        assert_eq!(
            store.dispatch_intents(&claim.effect_id).unwrap(),
            Some(intents)
        );
    }
}

#[test]
fn reservation_requires_a_known_frozen_hermes_intent() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claim = store.claim_effect("controller", 100, 30).unwrap().unwrap();
    assert!(
        store
            .reserve_dispatch_create(&claim, "planner:worker", 101)
            .is_err()
    );
    let mut intents = dispatch_intents();
    intents[0].transport = DispatchTransport::Direct;
    store
        .freeze_dispatch_intents(&claim, &intents, 101)
        .unwrap();
    assert!(
        store
            .reserve_dispatch_create(&claim, "unknown", 102)
            .is_err()
    );
    assert!(
        store
            .reserve_dispatch_create(&claim, "planner:worker", 102)
            .is_err()
    );
}

#[test]
fn dispatch_intents_and_create_attempts_cannot_be_edited_or_deleted() {
    let (_directory, mut store) = open();
    store.create_case(&new_case()).unwrap();
    let claim = store.claim_effect("controller", 100, 30).unwrap().unwrap();
    store
        .freeze_dispatch_intents(&claim, &dispatch_intents(), 101)
        .unwrap();
    store
        .reserve_dispatch_create(&claim, "planner:worker", 102)
        .unwrap();
    assert_eq!(store.status(103).unwrap().dispatch_create_attempts, 1);
    let connection = Connection::open(store.path()).unwrap();
    for sql in [
        "UPDATE dispatch_batches SET frozen_at = 0",
        "DELETE FROM dispatch_batches",
        "UPDATE dispatch_create_attempts SET attempted_at = 0",
        "DELETE FROM dispatch_create_attempts",
    ] {
        assert!(connection.execute(sql, []).is_err(), "{sql}");
    }
}

#[test]
fn terminal_workspace_retirement_is_eligible_once_and_immutably_audited() {
    let (_directory, mut store) = open();
    let mut case = new_case();
    case.initial_state = "COMPLETED".into();
    case.effects.clear();
    case.observed_at = 100;
    store.create_case(&case).unwrap();

    let candidates = store
        .terminal_workspace_candidates(984_321, 100, 10)
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].case_key, case.case_key);
    assert_eq!(candidates[0].updated_at, 100);

    let retirement = WorkspaceRetirementInput {
        case_key: case.case_key.clone(),
        state_revision: 1,
        worktree_path: "/var/lib/pip/worktrees/mdk/repo-984321-issue-1240-workflow-1".into(),
        outcome: WorkspaceRetirementOutcome::Retired,
        retired_at: 200,
    };
    assert_eq!(
        store.record_workspace_retirement(&retirement).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store.record_workspace_retirement(&retirement).unwrap(),
        ApplyResult::Replayed
    );
    assert!(
        store
            .terminal_workspace_candidates(984_321, 300, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.status(300).unwrap().workspace_retirements, 1);

    let connection = Connection::open(store.path()).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE workspace_retirements SET outcome = 'ABSENT' WHERE case_key = ?1",
                [&case.case_key],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM workspace_retirements WHERE case_key = ?1",
                [&case.case_key],
            )
            .is_err()
    );
}

#[test]
fn workspace_retirement_excludes_nonterminal_recent_and_running_cases() {
    let (_directory, mut store) = open();
    let mut nonterminal = new_case();
    nonterminal.case_key = "repo:984321#1@1".into();
    nonterminal.issue_number = 1;
    nonterminal.event.event_id = "event-nonterminal".into();
    nonterminal.effects.clear();
    nonterminal.observed_at = 10;
    store.create_case(&nonterminal).unwrap();

    let mut recent = new_case();
    recent.case_key = "repo:984321#2@1".into();
    recent.issue_number = 2;
    recent.initial_state = "ABANDONED".into();
    recent.event.event_id = "event-recent".into();
    recent.effects.clear();
    recent.observed_at = 200;
    store.create_case(&recent).unwrap();

    let mut running = new_case();
    running.case_key = "repo:984321#3@1".into();
    running.issue_number = 3;
    running.initial_state = "TAKEN_OVER".into();
    running.event.event_id = "event-running".into();
    running.effects[0].effect_id = "effect-running".into();
    running.observed_at = 10;
    store.create_case(&running).unwrap();
    let claimed = store.claim_effect("worker", 20, 30).unwrap().unwrap();
    assert_eq!(claimed.case_key, running.case_key);
    store
        .begin_direct_attempt(&claimed, "task-running", 21)
        .unwrap();

    assert!(
        store
            .terminal_workspace_candidates(984_321, 100, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn webhook_deliveries_are_immutable_idempotent_intake_evidence() {
    let (_directory, mut store) = open();
    let delivery = WebhookDeliveryInput {
        delivery_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
        repository_id: 984_321,
        event_name: "issues".into(),
        action: "labeled".into(),
        received_at: 1_787_000_000,
        payload_sha256: "a".repeat(64),
    };

    assert_eq!(
        store.record_webhook_delivery(&delivery).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store.record_webhook_delivery(&delivery).unwrap(),
        ApplyResult::Replayed
    );
    let mut conflict = delivery.clone();
    conflict.payload_sha256 = "b".repeat(64);
    assert!(matches!(
        store.record_webhook_delivery(&conflict),
        Err(StoreError::IdempotencyConflict { .. })
    ));

    let status = store.status(1_787_000_001).unwrap();
    assert_eq!(status.webhook_deliveries, 1);
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
    assert_eq!(status.schema_version, 8);
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
fn detached_review_observers_survive_transitions_and_record_once() {
    let (_directory, mut store) = open();
    let mut case = new_case();
    case.effects = vec![EffectInput {
        effect_id: "effect-shadow-opus".into(),
        effect_type: "RUN_DIRECT_OBSERVER".into(),
        payload: json!({"task_id": "secperf-opus-1"}),
    }];
    store.create_case(&case).unwrap();
    store.apply_transition(&transition(), None).unwrap();

    let claimed = store
        .claim_effect_matching("observer", 1_787_000_602, 30, &["RUN_DIRECT_OBSERVER"])
        .unwrap()
        .unwrap();
    let observation = ReviewObservationInput {
        observation_id: "observation-shadow-opus".into(),
        task_id: "secperf-opus-1".into(),
        reviewer_id: "secperf-opus".into(),
        role: "reviewer-secperf".into(),
        review_mode: "shadow".into(),
        plan_version: 1,
        review_round: 1,
        pr_number: 77,
        reviewed_head_sha: "b".repeat(40),
        payload: json!({"outcome": "APPROVE", "reviewer_id": "secperf-opus"}),
    };
    assert_eq!(
        store
            .complete_review_observation_effect(
                &claimed.effect_id,
                "observer",
                1_787_000_603,
                &observation,
            )
            .unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store
            .complete_review_observation_effect(
                &claimed.effect_id,
                "observer",
                1_787_000_604,
                &observation,
            )
            .unwrap(),
        ApplyResult::Replayed
    );
    let observations = store.review_observations_for_case(&case.case_key).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].reviewer_id, "secperf-opus");
    assert_eq!(observations[0].review_mode, "shadow");
    assert_eq!(store.status(1_787_000_604).unwrap().review_observations, 1);
    let conflicting = ReviewObservationInput {
        review_mode: "advisory".into(),
        ..observation
    };
    assert!(matches!(
        store.complete_review_observation_effect(
            &claimed.effect_id,
            "observer",
            1_787_000_605,
            &conflicting,
        ),
        Err(StoreError::IdempotencyConflict { .. })
    ));

    let connection = Connection::open(store.path()).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE review_observations SET reviewer_id = 'tampered' WHERE observation_id = 'observation-shadow-opus'",
                [],
            )
            .is_err()
    );
}

#[test]
fn authorization_removal_supersedes_a_detached_observer() {
    let (_directory, mut store) = open();
    let mut case = new_case();
    case.effects = vec![EffectInput {
        effect_id: "effect-shadow-opus".into(),
        effect_type: "RUN_DIRECT_OBSERVER".into(),
        payload: json!({"task_id": "secperf-opus-1"}),
    }];
    store.create_case(&case).unwrap();
    let mut removed = transition();
    removed.next_state = "ABANDONED".into();
    removed.event.event_id = "event-authorization-removed".into();
    removed.event.event_type = "AUTHORIZATION_REMOVED".into();
    store.apply_transition(&removed, None).unwrap();

    assert!(
        store
            .claim_effect_matching("observer", 1_787_000_602, 30, &["RUN_DIRECT_OBSERVER"])
            .unwrap()
            .is_none()
    );
    assert_eq!(store.status(1_787_000_602).unwrap().outbox_superseded, 1);
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
             DROP TABLE dispatch_create_attempts;
             DROP TABLE dispatch_batches;
             DROP TABLE review_observations;
             DROP TABLE webhook_deliveries;
             DROP TABLE workspace_retirements;
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
    assert_eq!(upgraded.schema_version().unwrap(), 8);
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
