use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use hmac::{Hmac, KeyInit, Mac};
use pip_control::{
    ActiveIntakeError, IntakeSource, RepositoryPolicy, WebhookEnvelope, ingest_webhook,
    load_repository_policy, reconcile_intake,
};
use pip_github::{
    GitHubError, IntakeSnapshot, IssueCommentSnapshot, IssueContentSnapshot, IssueSnapshot,
    LabelEvent, RepositorySnapshot,
};
use pip_store::Store;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

struct FixtureSource {
    discovered: Vec<IssueSnapshot>,
    snapshots: RefCell<BTreeMap<u64, IntakeSnapshot>>,
}

impl IntakeSource for FixtureSource {
    fn discover(
        &self,
        _owner: &str,
        _repository: &str,
        _label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError> {
        Ok(self.discovered.clone())
    }

    fn intake(
        &self,
        _owner: &str,
        _repository: &str,
        issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError> {
        self.snapshots
            .borrow()
            .get(&issue_number)
            .cloned()
            .ok_or_else(|| GitHubError::Transport("missing fixture".into()))
    }
}

#[test]
fn conversation_toggle_preserves_accepted_policy_and_active_work() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cases.db");
    let mut store = Store::open(&path).unwrap();
    let mut policy = active_policy(1, 1);
    let source = source(&[42]);
    reconcile_intake(&source, &policy, &mut store, 100, false).unwrap();
    let case_key = "repo:1055628515#42@3";
    let before = store.immutable_history_for_case(case_key).unwrap();
    let accepted_policy = || {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT payload_json, payload_sha256, accepted_at FROM policies",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap()
    };
    let snapshot = accepted_policy();
    for enabled in [true, false, true] {
        policy.conversations_enabled = enabled;
        assert_eq!(
            reconcile_intake(&source, &policy, &mut store, 101, false)
                .unwrap()
                .mutation_count,
            0
        );
        assert_eq!(accepted_policy(), snapshot);
        assert_eq!(store.immutable_history_for_case(case_key).unwrap(), before);
        assert!(
            pip_control::verify_active_authorization(&source, &policy, &store)
                .unwrap()
                .is_authorized()
        );
    }
    // The operational exception must not weaken immutable authorization/model policy.
    policy.intake.trusted_actor_ids.push(999);
    assert!(matches!(
        reconcile_intake(&source, &policy, &mut store, 102, false),
        Err(ActiveIntakeError::Store(
            pip_store::StoreError::IdempotencyConflict { .. }
        ))
    ));
    assert_eq!(accepted_policy(), snapshot);
}

#[test]
fn fresh_reauthorization_preserves_history_and_restarts_once_under_current_policy() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let mut policy = active_policy(1, 1);
    policy.max_case_elapsed_seconds = 60;
    let source = source(&[42]);
    reconcile_intake(&source, &policy, &mut store, 100, false).unwrap();
    abandon(&mut store, "AUTHORIZATION_REMOVED", "ABANDONED");
    let previous = store
        .immutable_history_for_case("repo:1055628515#42@3")
        .unwrap();
    relabel(&source);
    policy.revision += 1;
    let report = pip_control::reconcile_intake_with_quiescence(
        &source,
        &policy,
        &mut store,
        200,
        false,
        |_| true,
    )
    .unwrap();
    assert_eq!(report.candidates[0].decision, "ELIGIBLE");
    let case = store.case("repo:1055628515#42@3").unwrap().unwrap();
    assert_eq!(
        (
            case.state.as_str(),
            case.state_revision,
            case.policy_revision
        ),
        ("PLANNING", 3, policy.revision)
    );
    assert_eq!(
        store.reconstruct_case(&case.case_key).unwrap().unwrap(),
        case
    );
    let history = store.immutable_history_for_case(&case.case_key).unwrap();
    assert_eq!(store.case_created_at(&case.case_key).unwrap(), Some(100));
    assert_eq!(store.case_authorized_at(&case.case_key).unwrap(), Some(200));
    assert_eq!(&history.events[..2], previous.events.as_slice());
    assert_eq!(history.events[2].event_type, "ISSUE_REAUTHORIZED");
    assert_eq!(store.status(200).unwrap().outbox_pending, 1);
    let report = pip_control::reconcile_intake_with_quiescence(
        &source,
        &policy,
        &mut store,
        201,
        false,
        |_| true,
    )
    .unwrap();
    assert_eq!(report.mutation_count, 0);
    assert_eq!(
        store.immutable_history_for_case(&case.case_key).unwrap(),
        history
    );
    assert_eq!(
        pip_control::enforce_operational_bounds(&mut store, &policy, 259).unwrap(),
        pip_control::OperationalBoundsCycle::Idle
    );
    assert!(matches!(
        pip_control::enforce_operational_bounds(&mut store, &policy, 260).unwrap(),
        pip_control::OperationalBoundsCycle::Escalated {
            bound: pip_control::OperationalBound::ElapsedTime,
            observed: 60,
            ..
        }
    ));
}

#[test]
fn reauthorization_does_not_bypass_terminal_decisions_freshness_or_execution_guards() {
    for scenario in [
        "same-label",
        "no-removal",
        "untrusted",
        "runnable-task",
        "running-direct",
        "no-execution-observation",
        "existing-pr",
        "completed",
        "takeover",
        "explicit-abandon",
        "capacity",
        "paused",
    ] {
        let directory = tempdir().unwrap();
        let mut store = Store::open(directory.path().join("cases.db")).unwrap();
        let mut policy = active_policy(1, 1);
        let source = source(&[42]);
        reconcile_intake(&source, &policy, &mut store, 100, false).unwrap();
        if scenario == "running-direct" {
            let claim = store.claim_effect("old-worker", 101, 200).unwrap().unwrap();
            store.begin_direct_attempt(&claim, "old-task", 102).unwrap();
        }
        let (event, state) = match scenario {
            "completed" => ("HUMAN_MERGED", "COMPLETED"),
            "takeover" => ("HUMAN_TOOK_OVER", "TAKEN_OVER"),
            "explicit-abandon" => ("ABANDON", "ABANDONED"),
            _ => ("AUTHORIZATION_REMOVED", "ABANDONED"),
        };
        abandon(&mut store, event, state);
        if scenario == "existing-pr" {
            // A withdrawal after publication must never start an unrelated PR.
            let connection = rusqlite::Connection::open(directory.path().join("cases.db")).unwrap();
            connection
                .execute("UPDATE cases SET pr_number=123 WHERE issue_number=42", [])
                .unwrap();
        }
        if scenario != "same-label" {
            relabel(&source);
        }
        if scenario == "no-removal" {
            source
                .snapshots
                .borrow_mut()
                .get_mut(&42)
                .unwrap()
                .label_events
                .retain(|event| event.labeled);
        }
        if scenario == "untrusted" {
            source
                .snapshots
                .borrow_mut()
                .get_mut(&42)
                .unwrap()
                .label_events
                .last_mut()
                .unwrap()
                .actor_id = 999;
        }
        if scenario == "capacity" {
            let peer = self::source(&[99]);
            reconcile_intake(&peer, &policy, &mut store, 120, false).unwrap();
        }
        let before = store
            .immutable_history_for_case("repo:1055628515#42@3")
            .unwrap();
        if scenario == "paused" {
            policy.intake.paused = true;
        }
        let report = if scenario == "no-execution-observation" {
            reconcile_intake(&source, &policy, &mut store, 200, false)
        } else {
            pip_control::reconcile_intake_with_quiescence(
                &source,
                &policy,
                &mut store,
                200,
                false,
                |_| scenario != "runnable-task",
            )
        };
        if scenario == "paused" {
            assert!(report.is_err());
        } else {
            assert_eq!(
                report.unwrap().candidates[0].decision,
                "INELIGIBLE",
                "{scenario}"
            );
        }
        assert_eq!(
            store
                .immutable_history_for_case("repo:1055628515#42@3")
                .unwrap(),
            before,
            "{scenario}"
        );
    }
}

fn abandon(store: &mut Store, event_type: &str, state: &str) {
    store.apply_transition(&pip_store::TransitionInput {
        case_key: "repo:1055628515#42@3".into(), expected_revision: 1,
        next_state: state.into(), remediation_round: 0, plan_version: 0,
        pr_number: None, head_sha: None, observed_at: 110,
        event: pip_store::EventInput { event_id: "withdrawal".into(), event_type: event_type.into(),
            payload: serde_json::json!({"blockers":["REQUIRED_LABEL_MISSING","LATEST_AUTHORIZATION_REMOVED"]}) },
        run: None, evidence: vec![], findings: vec![], effects: vec![],
    }, None).unwrap();
}

fn relabel(source: &FixtureSource) {
    let mut snapshots = source.snapshots.borrow_mut();
    let snapshot = snapshots.get_mut(&42).unwrap();
    snapshot.label_events.extend([
        LabelEvent {
            id: 20_042,
            labeled: false,
            actor_id: 202880,
            label: "pip-ok".into(),
            created_at: "2026-08-21T12:00:00Z".into(),
        },
        LabelEvent {
            id: 30_042,
            labeled: true,
            actor_id: 202880,
            label: "pip-ok".into(),
            created_at: "2026-08-22T12:00:00Z".into(),
        },
    ]);
}

#[test]
fn intake_freezes_bounded_issue_context_for_credential_free_workers() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let policy = active_policy(1, 1);
    let source = source(&[42]);
    let comment = IssueCommentSnapshot {
        id: 501,
        actor_id: 202880,
        issue_number: 42,
        html_url: "https://github.com/marmot-protocol/mdk/issues/42#issuecomment-501".into(),
        body: "Untrusted issue discussion".into(),
        body_sha256: Sha256::digest(b"Untrusted issue discussion")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        created_at: "2026-09-05T00:00:00Z".into(),
        updated_at: "2026-09-05T00:01:00Z".into(),
    };
    source
        .snapshots
        .borrow_mut()
        .get_mut(&42)
        .unwrap()
        .comments
        .push(comment.clone());
    reconcile_intake(&source, &policy, &mut store, 100, false).unwrap();
    let history = store
        .immutable_history_for_case("repo:1055628515#42@3")
        .unwrap();
    let context = &history.events[0].payload["issue_context"];
    assert_eq!(context["schema_version"], 1);
    assert_eq!(context["observed_at"], 100);
    assert_eq!(context["repository"]["full_name"], "marmot-protocol/mdk");
    assert_eq!(context["issue"]["number"], 42);
    assert_eq!(context["issue_content"]["body"], "Fixture body");
    assert_eq!(context["comments"], serde_json::json!([comment]));
    source
        .snapshots
        .borrow_mut()
        .get_mut(&42)
        .unwrap()
        .issue_content
        .body = "changed later".into();
    reconcile_intake(&source, &policy, &mut store, 101, false).unwrap();
    assert_eq!(
        store
            .immutable_history_for_case("repo:1055628515#42@3")
            .unwrap(),
        history
    );
}

#[test]
fn oversized_issue_context_cannot_create_case_or_dispatch() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let source = source(&[42]);
    source
        .snapshots
        .borrow_mut()
        .get_mut(&42)
        .unwrap()
        .issue_content
        .body = "x".repeat(256 * 1024);
    assert!(reconcile_intake(&source, &active_policy(1, 1), &mut store, 100, false).is_err());
    let status = store.status(100).unwrap();
    assert!(status.cases.is_empty());
    assert_eq!(status.events, 0);
    assert_eq!(status.outbox_total, 0);
}

#[test]
fn eligible_issue_creates_one_planning_case_and_replays_without_duplicates() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let policy = active_policy(2, 2);
    let source = source(&[42]);

    let first = reconcile_intake(&source, &policy, &mut store, 100, false).unwrap();
    assert_eq!(first.mutation_count, 2);
    assert_eq!(first.candidates[0].decision, "ELIGIBLE");
    assert_eq!(
        first.candidates[0].case_key.as_deref(),
        Some("repo:1055628515#42@3")
    );

    let stored = store.case("repo:1055628515#42@3").unwrap().unwrap();
    assert_eq!(stored.state, "PLANNING");
    assert_eq!(stored.repository_id, 1_055_628_515);
    assert_eq!(stored.issue_number, 42);
    assert_eq!(stored.workflow_version, 3);
    let status = store.status(100).unwrap();
    assert_eq!(status.cases.len(), 1);
    assert_eq!(status.events, 1);
    assert_eq!(status.outbox_pending, 1);

    let replay = reconcile_intake(&source, &policy, &mut store, 101, false).unwrap();
    assert_eq!(replay.mutation_count, 0);
    assert_eq!(replay.candidates[0].decision, "INELIGIBLE");
    assert_eq!(replay.candidates[0].blockers, ["ALREADY_OWNED"]);
    let status = store.status(101).unwrap();
    assert_eq!(status.cases.len(), 1);
    assert_eq!(status.events, 1);
    assert_eq!(status.outbox_pending, 1);
}

#[test]
fn capacity_is_applied_in_sorted_issue_order() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let policy = active_policy(1, 1);
    let source = source(&[99, 42]);

    let report = reconcile_intake(&source, &policy, &mut store, 100, false).unwrap();

    assert_eq!(report.candidates[0].issue_number, 42);
    assert_eq!(report.candidates[0].decision, "ELIGIBLE");
    assert_eq!(report.candidates[1].issue_number, 99);
    assert_eq!(report.candidates[1].decision, "INELIGIBLE");
    assert_eq!(
        report.candidates[1].blockers,
        ["REPOSITORY_LIMIT_REACHED", "GLOBAL_LIMIT_REACHED"]
    );
    assert!(store.case("repo:1055628515#42@3").unwrap().is_some());
    assert!(store.case("repo:1055628515#99@3").unwrap().is_none());
}

#[test]
fn policy_held_issue_is_never_created() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let mut policy = active_policy(1, 1);
    policy.intake.held_issue_numbers = vec![42];

    let report = reconcile_intake(&source(&[42]), &policy, &mut store, 100, false).unwrap();
    assert_eq!(report.candidates[0].decision, "INELIGIBLE");
    assert_eq!(report.candidates[0].blockers, ["HELD"]);
    assert!(store.status(100).unwrap().cases.is_empty());
}

#[test]
fn disabled_or_paused_activation_does_not_touch_the_ledger() {
    for configure in [
        |policy: &mut RepositoryPolicy| policy.intake.enabled = false,
        |policy: &mut RepositoryPolicy| policy.intake.paused = true,
        |policy: &mut RepositoryPolicy| policy.dispatch_enabled = false,
    ] {
        let directory = tempdir().unwrap();
        let mut store = Store::open(directory.path().join("cases.db")).unwrap();
        let mut policy = active_policy(1, 1);
        configure(&mut policy);

        assert!(matches!(
            reconcile_intake(&source(&[42]), &policy, &mut store, 100, false),
            Err(ActiveIntakeError::ActivationDisabled)
        ));
        let status = store.status(100).unwrap();
        assert!(status.cases.is_empty());
        assert_eq!(status.events, 0);
        assert_eq!(status.outbox_total, 0);
    }
}

#[test]
fn repository_or_discovery_drift_creates_no_case() {
    let policy = active_policy(1, 1);
    for mutate in [
        |snapshot: &mut IntakeSnapshot| snapshot.repository.id = 9,
        |snapshot: &mut IntakeSnapshot| snapshot.issue.id = 9,
    ] {
        let directory = tempdir().unwrap();
        let mut store = Store::open(directory.path().join("cases.db")).unwrap();
        let source = source(&[42]);
        mutate(source.snapshots.borrow_mut().get_mut(&42).unwrap());

        assert!(reconcile_intake(&source, &policy, &mut store, 100, false).is_err());
        let status = store.status(100).unwrap();
        assert!(status.cases.is_empty());
        assert_eq!(status.events, 0);
        assert_eq!(status.outbox_total, 0);
    }
}

#[test]
fn signed_label_webhook_routes_one_issue_and_replays_by_delivery_id() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let policy = active_policy(1, 1);
    let source = source(&[42]);
    let payload = webhook_payload(42, "pip-ok", 1_055_628_515);
    let signature = signature(b"webhook-secret", &payload);
    let envelope = WebhookEnvelope {
        delivery_id: "01234567-89ab-cdef-0123-456789abcdef",
        event_name: "issues",
        signature: &signature,
        payload: &payload,
        received_at: 100,
    };

    let first = ingest_webhook(
        &source,
        &policy,
        &mut store,
        envelope,
        b"webhook-secret",
        100,
        false,
    )
    .unwrap();
    assert_eq!(first.delivery, "APPLIED");
    assert_eq!(first.candidate.unwrap().decision, "ELIGIBLE");
    assert!(store.case("repo:1055628515#42@3").unwrap().is_some());

    let replay = ingest_webhook(
        &source,
        &policy,
        &mut store,
        envelope,
        b"webhook-secret",
        101,
        false,
    )
    .unwrap();
    assert_eq!(replay.delivery, "REPLAYED");
    assert_eq!(replay.candidate.unwrap().decision, "INELIGIBLE");
    assert_eq!(store.status(101).unwrap().webhook_deliveries, 1);
}

#[test]
fn superseded_signed_label_webhook_is_acknowledged_from_latest_live_state() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let policy = active_policy(1, 1);
    let source = source(&[42]);
    {
        let mut snapshots = source.snapshots.borrow_mut();
        let snapshot = snapshots.get_mut(&42).unwrap();
        snapshot.issue.labels.clear();
        snapshot.label_events.push(LabelEvent {
            id: 10_043,
            labeled: false,
            actor_id: 202_880,
            label: "pip-ok".into(),
            created_at: "2026-08-20T12:01:00Z".into(),
        });
    }
    let payload = webhook_payload(42, "pip-ok", 1_055_628_515);
    let signature = signature(b"webhook-secret", &payload);

    let report = ingest_webhook(
        &source,
        &policy,
        &mut store,
        WebhookEnvelope {
            delivery_id: "02234567-89ab-cdef-0123-456789abcdef",
            event_name: "issues",
            signature: &signature,
            payload: &payload,
            received_at: 100,
        },
        b"webhook-secret",
        101,
        false,
    )
    .unwrap();

    assert_eq!(report.delivery, "APPLIED");
    let candidate = report.candidate.unwrap();
    assert_eq!(candidate.decision, "INELIGIBLE");
    assert_eq!(candidate.blockers, ["REQUIRED_LABEL_MISSING"]);
    let status = store.status(101).unwrap();
    assert_eq!(status.webhook_deliveries, 1);
    assert!(status.cases.is_empty());
    assert_eq!(status.events, 0);
    assert_eq!(status.outbox_total, 0);
}

#[test]
fn signed_label_webhook_sender_must_exist_in_live_label_history() {
    let directory = tempdir().unwrap();
    let mut store = Store::open(directory.path().join("cases.db")).unwrap();
    let policy = active_policy(1, 1);
    let source = source(&[42]);
    source
        .snapshots
        .borrow_mut()
        .get_mut(&42)
        .unwrap()
        .label_events[0]
        .actor_id = 999;
    let payload = webhook_payload(42, "pip-ok", 1_055_628_515);
    let signature = signature(b"webhook-secret", &payload);

    let result = ingest_webhook(
        &source,
        &policy,
        &mut store,
        WebhookEnvelope {
            delivery_id: "03234567-89ab-cdef-0123-456789abcdef",
            event_name: "issues",
            signature: &signature,
            payload: &payload,
            received_at: 100,
        },
        b"webhook-secret",
        101,
        false,
    );

    assert!(matches!(result, Err(ActiveIntakeError::Evidence(_))));
    let status = store.status(101).unwrap();
    assert_eq!(status.webhook_deliveries, 1);
    assert!(status.cases.is_empty());
    assert_eq!(status.events, 0);
    assert_eq!(status.outbox_total, 0);
}

#[test]
fn webhook_fails_closed_before_recording_or_fetching_on_bad_signature_or_repository() {
    let policy = active_policy(1, 1);
    for (payload, signature) in [
        (
            webhook_payload(42, "pip-ok", 1_055_628_515),
            "sha256=00".into(),
        ),
        {
            let payload = webhook_payload(42, "pip-ok", 9);
            let signature = signature(b"webhook-secret", &payload);
            (payload, signature)
        },
        {
            let payload = serde_json::to_vec(&serde_json::json!({
                "action": "labeled",
                "repository": {
                    "id": 1_055_628_515_u64,
                    "full_name": "marmot-protocol/mdk"
                },
                "issue": {"id": 542, "number": 42},
                "sender": {"id": 202_880}
            }))
            .unwrap();
            let signature = signature(b"webhook-secret", &payload);
            (payload, signature)
        },
    ] {
        let directory = tempdir().unwrap();
        let mut store = Store::open(directory.path().join("cases.db")).unwrap();
        let result = ingest_webhook(
            &source(&[42]),
            &policy,
            &mut store,
            WebhookEnvelope {
                delivery_id: "01234567-89ab-cdef-0123-456789abcdef",
                event_name: "issues",
                signature: &signature,
                payload: &payload,
                received_at: 100,
            },
            b"webhook-secret",
            100,
            false,
        );
        assert!(result.is_err());
        let status = store.status(100).unwrap();
        assert_eq!(status.webhook_deliveries, 0);
        assert!(status.cases.is_empty());
    }
}

fn active_policy(repository_limit: u32, global_limit: u32) -> RepositoryPolicy {
    let mut policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy.intake.enabled = true;
    policy.intake.paused = false;
    policy.dispatch_enabled = true;
    policy.github.automation_actor_id = Some(202_880);
    policy.intake.trusted_actor_ids = vec![202_880];
    policy.intake.excluded_issue_numbers.clear();
    policy.intake.repository_active_limit = repository_limit;
    policy.intake.global_active_limit = global_limit;
    policy
}

fn source(issue_numbers: &[u64]) -> FixtureSource {
    let discovered = issue_numbers.iter().copied().map(issue).collect::<Vec<_>>();
    let snapshots = discovered
        .iter()
        .cloned()
        .map(|issue| {
            (
                issue.number,
                IntakeSnapshot {
                    repository: RepositorySnapshot {
                        id: 1_055_628_515,
                        full_name: "marmot-protocol/mdk".into(),
                        default_branch: "master".into(),
                    },
                    issue_content: IssueContentSnapshot {
                        author_id: 1001,
                        title: format!("Fixture issue #{}", issue.number),
                        body: "Fixture body".into(),
                        created_at: "2026-08-19T00:00:00Z".into(),
                        updated_at: "2026-08-20T00:00:00Z".into(),
                    },
                    label_events: vec![LabelEvent {
                        id: 10_000 + issue.number,
                        labeled: true,
                        actor_id: 202_880,
                        label: "pip-ok".into(),
                        created_at: "2026-08-20T12:00:00Z".into(),
                    }],
                    comments: Vec::new(),
                    issue,
                },
            )
        })
        .collect();
    FixtureSource {
        discovered,
        snapshots: RefCell::new(snapshots),
    }
}

fn issue(number: u64) -> IssueSnapshot {
    IssueSnapshot {
        id: 500 + number,
        number,
        open: true,
        is_pull_request: false,
        labels: BTreeSet::from(["pip-ok".into()]),
    }
}

fn webhook_payload(issue: u64, label: &str, repository_id: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "action": "labeled",
        "repository": {
            "id": repository_id,
            "full_name": "marmot-protocol/mdk"
        },
        "issue": {
            "id": 500 + issue,
            "number": issue
        },
        "label": {"name": label},
        "sender": {"id": 202880}
    }))
    .unwrap()
}

fn signature(secret: &[u8], payload: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(payload);
    format!(
        "sha256={}",
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}
