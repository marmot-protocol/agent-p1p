use std::cell::RefCell;

use pip_control::{
    DispositionCycle, DispositionWriter, consume_disposition_once, load_repository_policy,
};
use pip_github::{CommentSpec, GitHubError, MutationResult};
use pip_store::{EffectInput, EventInput, NewCase, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Default)]
struct FixtureWriter {
    comments: RefCell<Vec<CommentSpec>>,
    fail: bool,
}

impl DispositionWriter for FixtureWriter {
    fn ensure_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError> {
        self.comments.borrow_mut().push(spec.clone());
        if self.fail {
            return Err(GitHubError::Transport("fixture outage".into()));
        }
        Ok(MutationResult::Created(9001))
    }
}

#[test]
fn github_failure_releases_the_effect_for_immediate_idempotent_retry() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = disposition_store(
        directory.path().join("ledger.db"),
        "SHADOW_READY",
        "NOTIFY_SHADOW_READY",
    );
    let writer = FixtureWriter {
        fail: true,
        ..FixtureWriter::default()
    };
    assert!(
        consume_disposition_once(
            &writer,
            &active_policy(),
            &mut store,
            100,
            "disposition",
            30,
            true,
        )
        .is_err()
    );
    assert!(
        store
            .claim_effect_matching("retry", 100, 30, &["NOTIFY_SHADOW_READY"])
            .unwrap()
            .is_some()
    );
    assert_eq!(store.evidence_count().unwrap(), 0);
}

#[test]
fn shadow_ready_publishes_one_human_held_pr_comment_and_records_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = disposition_store(
        directory.path().join("ledger.db"),
        "SHADOW_READY",
        "NOTIFY_SHADOW_READY",
    );
    let writer = FixtureWriter::default();

    let result = consume_disposition_once(
        &writer,
        &active_policy(),
        &mut store,
        100,
        "disposition",
        30,
        true,
    )
    .unwrap();
    assert_eq!(
        result,
        DispositionCycle::Published {
            effect_id: "effect-disposition".into(),
            target_number: 77,
            external_id: 9001,
        }
    );
    let comments = writer.comments.borrow();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].issue_number, 77);
    assert!(comments[0].body.contains("human merge"));
    assert!(comments[0].body.contains(&"b".repeat(40)));
    assert_eq!(store.evidence_count().unwrap(), 1);
    assert_eq!(store.status(100).unwrap().outbox_delivered, 1);
}

#[test]
fn invalid_authorization_leaves_comment_effect_unclaimed() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = disposition_store(
        directory.path().join("ledger.db"),
        "WAITING_HUMAN",
        "HOLD_FOR_HUMAN",
    );
    let writer = FixtureWriter::default();
    assert_eq!(
        consume_disposition_once(
            &writer,
            &active_policy(),
            &mut store,
            100,
            "disposition",
            30,
            false,
        )
        .unwrap(),
        DispositionCycle::AuthorizationBlocked
    );
    assert!(writer.comments.borrow().is_empty());
    assert!(
        store
            .claim_effect_matching("retry", 100, 30, &["HOLD_FOR_HUMAN"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn takeover_record_is_consumed_locally_without_a_github_write() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = disposition_store(
        directory.path().join("ledger.db"),
        "TAKEN_OVER",
        "RECORD_TAKEOVER",
    );
    let writer = FixtureWriter::default();
    assert_eq!(
        consume_disposition_once(
            &writer,
            &active_policy(),
            &mut store,
            100,
            "disposition",
            30,
            false,
        )
        .unwrap(),
        DispositionCycle::Recorded {
            effect_id: "effect-disposition".into(),
            effect_type: "RECORD_TAKEOVER".into(),
        }
    );
    assert!(writer.comments.borrow().is_empty());
    assert_eq!(store.evidence_count().unwrap(), 1);
}

fn disposition_store(path: std::path::PathBuf, state: &str, effect_type: &str) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label":"pip-ok"}),
            },
            effects: vec![EffectInput {
                effect_id: "effect-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key":"repo:984321#1240@1"}),
            }],
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 1,
                next_state: state.into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 2,
                event: EventInput {
                    event_id: "event-disposition".into(),
                    event_type: "FIXTURE_DISPOSITION".into(),
                    payload: json!({"state":state}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-disposition".into(),
                    effect_type: effect_type.into(),
                    payload: json!({
                        "case_key":"repo:984321#1240@1",
                        "state_revision":2,
                        "effect":effect_type,
                        "pr_number":77,
                        "head_sha":"b".repeat(40),
                    }),
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn active_policy() -> pip_control::RepositoryPolicy {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["repository"]["id"] = json!(984321);
    value["workflow_version"] = json!(1);
    value["intake"]["enabled"] = json!(true);
    value["intake"]["paused"] = json!(false);
    value["dispatch_enabled"] = json!(true);
    value["github"]["automation_actor_id"] = json!(202880);
    value["github"]["reviewer_general_actor_id"] = json!(202881);
    value["github"]["reviewer_secperf_actor_id"] = json!(202882);
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}
