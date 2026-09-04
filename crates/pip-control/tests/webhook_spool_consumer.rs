use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use hmac::{Hmac, KeyInit, Mac};
use pip_control::{
    IntakeSource, RepositoryPolicy, WebhookSpool, WebhookSpoolInput, consume_webhook_spool_once,
    load_repository_policy,
};
use pip_github::{
    GitHubError, IntakeSnapshot, IssueContentSnapshot, IssueSnapshot, LabelEvent,
    RepositorySnapshot,
};
use pip_store::Store;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

struct FixtureSource {
    snapshots: RefCell<BTreeMap<u64, IntakeSnapshot>>,
    fail: bool,
}

impl IntakeSource for FixtureSource {
    fn discover(
        &self,
        _owner: &str,
        _repository: &str,
        _label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError> {
        unreachable!("spool consumption re-reads one exact issue")
    }

    fn intake(
        &self,
        _owner: &str,
        _repository: &str,
        issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError> {
        if self.fail {
            return Err(GitHubError::Transport("fixture outage".into()));
        }
        self.snapshots
            .borrow()
            .get(&issue_number)
            .cloned()
            .ok_or_else(|| GitHubError::Transport("missing fixture".into()))
    }
}

#[test]
fn signed_spool_item_is_committed_before_it_is_marked_processed() {
    let directory = tempfile::tempdir().unwrap();
    let spool_root = directory.path().join("spool");
    prepare_spool(&spool_root);
    let spool = WebhookSpool::open(&spool_root).unwrap();
    let payload = webhook_payload(42);
    let signature = signature(b"webhook-secret", &payload);
    spool
        .store(
            WebhookSpoolInput {
                delivery_id: "01234567-89ab-cdef-0123-456789abcdef",
                event_name: "issues",
                signature: &signature,
                payload: &payload,
                received_at: 100,
            },
            b"webhook-secret",
        )
        .unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();

    let result = consume_webhook_spool_once(
        &source(false),
        &active_policy(),
        &mut store,
        &spool,
        b"webhook-secret",
        101,
        false,
    )
    .unwrap();

    assert_eq!(result.result, "PROCESSED");
    assert_eq!(
        result.delivery_id.as_deref(),
        Some("01234567-89ab-cdef-0123-456789abcdef")
    );
    assert_eq!(result.intake.unwrap().delivery, "APPLIED");
    assert!(store.case("repo:1055628515#42@3").unwrap().is_some());
    assert_eq!(store.status(101).unwrap().webhook_deliveries, 1);
    assert!(
        !spool_root
            .join("pending/01234567-89ab-cdef-0123-456789abcdef.json")
            .exists()
    );
    assert!(
        spool_root
            .join("processed/01234567-89ab-cdef-0123-456789abcdef.json")
            .is_file()
    );

    let empty = consume_webhook_spool_once(
        &source(false),
        &active_policy(),
        &mut store,
        &spool,
        b"webhook-secret",
        102,
        false,
    )
    .unwrap();
    assert_eq!(empty.result, "EMPTY");
    assert!(empty.delivery_id.is_none());
}

#[test]
fn paused_intake_commits_the_delivery_without_creating_or_dispatching_a_case() {
    let directory = tempfile::tempdir().unwrap();
    let spool_root = directory.path().join("spool");
    prepare_spool(&spool_root);
    let spool = WebhookSpool::open(&spool_root).unwrap();
    let payload = webhook_payload(42);
    let signature = signature(b"webhook-secret", &payload);
    let delivery_id = "02234567-89ab-cdef-0123-456789abcdef";
    spool
        .store(
            WebhookSpoolInput {
                delivery_id,
                event_name: "issues",
                signature: &signature,
                payload: &payload,
                received_at: 100,
            },
            b"webhook-secret",
        )
        .unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();

    let result = consume_webhook_spool_once(
        &source(false),
        &inactive_policy(),
        &mut store,
        &spool,
        b"webhook-secret",
        101,
        false,
    )
    .unwrap();

    assert_eq!(result.result, "PROCESSED");
    let intake = result.intake.unwrap();
    assert_eq!(intake.delivery, "APPLIED");
    let candidate = intake.candidate.unwrap();
    assert_eq!(candidate.decision, "INELIGIBLE");
    assert_eq!(
        candidate.blockers,
        ["INTAKE_DISABLED", "REPOSITORY_PAUSED", "DISPATCH_DISABLED"]
    );
    let status = store.status(101).unwrap();
    assert_eq!(status.webhook_deliveries, 1);
    assert!(status.cases.is_empty());
    assert_eq!(status.outbox_total, 0);
    assert!(
        !spool_root
            .join(format!("pending/{delivery_id}.json"))
            .exists()
    );
    assert!(
        spool_root
            .join(format!("processed/{delivery_id}.json"))
            .is_file()
    );
}

#[test]
fn unrelated_signed_issue_action_is_committed_without_a_live_issue_read() {
    let directory = tempfile::tempdir().unwrap();
    let spool_root = directory.path().join("spool");
    prepare_spool(&spool_root);
    let spool = WebhookSpool::open(&spool_root).unwrap();
    let payload = serde_json::to_vec(&serde_json::json!({
        "action": "closed",
        "repository": {
            "id": 1055628515_u64,
            "full_name": "marmot-protocol/mdk"
        },
        "issue": {"id": 542, "number": 42},
        "sender": {"id": 202880}
    }))
    .unwrap();
    let signature = signature(b"webhook-secret", &payload);
    let delivery_id = "03234567-89ab-cdef-0123-456789abcdef";
    spool
        .store(
            WebhookSpoolInput {
                delivery_id,
                event_name: "issues",
                signature: &signature,
                payload: &payload,
                received_at: 100,
            },
            b"webhook-secret",
        )
        .unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();

    let result = consume_webhook_spool_once(
        &source(true),
        &inactive_policy(),
        &mut store,
        &spool,
        b"webhook-secret",
        101,
        false,
    )
    .unwrap();

    assert_eq!(result.result, "PROCESSED");
    let intake = result.intake.unwrap();
    assert_eq!(intake.delivery, "APPLIED");
    assert!(intake.candidate.is_none());
    let status = store.status(101).unwrap();
    assert_eq!(status.webhook_deliveries, 1);
    assert!(status.cases.is_empty());
    assert_eq!(status.outbox_total, 0);
    assert!(
        !spool_root
            .join(format!("pending/{delivery_id}.json"))
            .exists()
    );
    assert!(
        spool_root
            .join(format!("processed/{delivery_id}.json"))
            .is_file()
    );
}

#[test]
fn live_github_failure_leaves_the_item_pending_for_a_replay_safe_retry() {
    let directory = tempfile::tempdir().unwrap();
    let spool_root = directory.path().join("spool");
    prepare_spool(&spool_root);
    let spool = WebhookSpool::open(&spool_root).unwrap();
    let payload = webhook_payload(42);
    let signature = signature(b"webhook-secret", &payload);
    spool
        .store(
            WebhookSpoolInput {
                delivery_id: "11234567-89ab-cdef-0123-456789abcdef",
                event_name: "issues",
                signature: &signature,
                payload: &payload,
                received_at: 100,
            },
            b"webhook-secret",
        )
        .unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();

    assert!(
        consume_webhook_spool_once(
            &source(true),
            &active_policy(),
            &mut store,
            &spool,
            b"webhook-secret",
            101,
            false,
        )
        .is_err()
    );

    assert!(
        spool_root
            .join("pending/11234567-89ab-cdef-0123-456789abcdef.json")
            .is_file()
    );
    assert!(
        !spool_root
            .join("processed/11234567-89ab-cdef-0123-456789abcdef.json")
            .exists()
    );

    let retry = consume_webhook_spool_once(
        &source(false),
        &active_policy(),
        &mut store,
        &spool,
        b"webhook-secret",
        102,
        false,
    )
    .unwrap();
    assert_eq!(retry.intake.unwrap().delivery, "REPLAYED");
    assert_eq!(store.status(102).unwrap().webhook_deliveries, 1);
}

#[test]
fn a_tampered_spool_envelope_is_never_read_as_a_webhook() {
    let directory = tempfile::tempdir().unwrap();
    let spool_root = directory.path().join("spool");
    prepare_spool(&spool_root);
    let spool = WebhookSpool::open(&spool_root).unwrap();
    let payload = webhook_payload(42);
    let signature = signature(b"webhook-secret", &payload);
    let delivery_id = "21234567-89ab-cdef-0123-456789abcdef";
    spool
        .store(
            WebhookSpoolInput {
                delivery_id,
                event_name: "issues",
                signature: &signature,
                payload: &payload,
                received_at: 100,
            },
            b"webhook-secret",
        )
        .unwrap();
    let pending = spool_root.join(format!("pending/{delivery_id}.json"));
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&pending).unwrap()).unwrap();
    envelope["payload_sha256"] = serde_json::json!("0".repeat(64));
    fs::write(&pending, serde_json::to_vec(&envelope).unwrap()).unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();

    assert!(
        consume_webhook_spool_once(
            &source(false),
            &active_policy(),
            &mut store,
            &spool,
            b"webhook-secret",
            101,
            false,
        )
        .is_err()
    );
    assert_eq!(store.status(101).unwrap().webhook_deliveries, 0);
    assert!(pending.exists());
}

fn prepare_spool(root: &std::path::Path) {
    for child in ["receipts", "pending", "processed"] {
        fs::create_dir_all(root.join(child)).unwrap();
    }
}

fn active_policy() -> RepositoryPolicy {
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
    policy
}

fn inactive_policy() -> RepositoryPolicy {
    let mut policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy.github.automation_actor_id = Some(202_880);
    policy.intake.trusted_actor_ids = vec![202_880];
    policy.intake.excluded_issue_numbers.clear();
    policy
}

fn source(fail: bool) -> FixtureSource {
    let issue = IssueSnapshot {
        id: 542,
        number: 42,
        open: true,
        is_pull_request: false,
        labels: BTreeSet::from(["pip-ok".into()]),
    };
    FixtureSource {
        snapshots: RefCell::new(BTreeMap::from([(
            42,
            IntakeSnapshot {
                repository: RepositorySnapshot {
                    id: 1_055_628_515,
                    full_name: "marmot-protocol/mdk".into(),
                    default_branch: "master".into(),
                },
                issue_content: IssueContentSnapshot {
                    author_id: 1001,
                    title: "Fixture issue #42".into(),
                    body: "Fixture body".into(),
                    created_at: "2026-08-19T00:00:00Z".into(),
                    updated_at: "2026-08-20T00:00:00Z".into(),
                },
                label_events: vec![LabelEvent {
                    id: 10_042,
                    labeled: true,
                    actor_id: 202_880,
                    label: "pip-ok".into(),
                    created_at: "2026-08-20T12:00:00Z".into(),
                }],
                comments: Vec::new(),
                issue,
            },
        )])),
        fail,
    }
}

fn webhook_payload(issue: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "action": "labeled",
        "repository": {
            "id": 1055628515_u64,
            "full_name": "marmot-protocol/mdk"
        },
        "issue": {"id": 500 + issue, "number": issue},
        "label": {"name": "pip-ok"},
        "sender": {"id": 202880}
    }))
    .unwrap()
}

fn signature(secret: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).unwrap();
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
