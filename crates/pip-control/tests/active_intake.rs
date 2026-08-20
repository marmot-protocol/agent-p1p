use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use hmac::{Hmac, KeyInit, Mac};
use pip_control::{
    ActiveIntakeError, IntakeSource, RepositoryPolicy, WebhookEnvelope, ingest_webhook,
    load_repository_policy, reconcile_intake,
};
use pip_github::{
    GitHubError, IntakeSnapshot, IssueContentSnapshot, IssueSnapshot, LabelEvent,
    RepositorySnapshot,
};
use pip_store::Store;
use sha2::Sha256;
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
        Some("repo:1055628515#42@2")
    );

    let stored = store.case("repo:1055628515#42@2").unwrap().unwrap();
    assert_eq!(stored.state, "PLANNING");
    assert_eq!(stored.repository_id, 1_055_628_515);
    assert_eq!(stored.issue_number, 42);
    assert_eq!(stored.workflow_version, 2);
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
    assert!(store.case("repo:1055628515#42@2").unwrap().is_some());
    assert!(store.case("repo:1055628515#99@2").unwrap().is_none());
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
    assert!(store.case("repo:1055628515#42@2").unwrap().is_some());

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
