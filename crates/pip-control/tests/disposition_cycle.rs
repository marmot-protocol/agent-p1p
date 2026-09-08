use std::cell::RefCell;

use pip_control::{
    DispositionCycle, DispositionWriter, consume_disposition_once, load_repository_policy,
};
use pip_github::{CommentSpec, GitHubError, MutationResult};
use pip_store::{EffectInput, EventInput, NewCase, Store, TransitionInput};
use serde_json::{Value, json};

struct NoReads;
impl pip_github::ReadTransport for NoReads {
    fn get(&self, _: pip_github::ReadRequest) -> Result<pip_github::ReadResponse, GitHubError> {
        panic!("non-ready dispositions do not need readiness reads")
    }
}

fn unused_source() -> pip_github::GitHubReader<NoReads> {
    pip_github::GitHubReader::new(NoReads, "https://github.test", "fixture", 1024, 1).unwrap()
}

#[derive(Default)]
struct FixtureWriter {
    comments: RefCell<Vec<CommentSpec>>,
    fail: bool,
}

impl DispositionWriter for FixtureWriter {
    fn mark_ready(
        &self,
        _: &pip_github::PullRequestReadySpec,
    ) -> Result<MutationResult, GitHubError> {
        panic!("non-ready dispositions must not change draft status")
    }
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
        "WAITING_HUMAN",
        "HOLD_FOR_HUMAN",
    );
    let writer = FixtureWriter {
        fail: true,
        ..FixtureWriter::default()
    };
    assert!(
        consume_disposition_once(
            (&unused_source(), &writer),
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
            .claim_effect_matching("retry", 100, 30, &["HOLD_FOR_HUMAN"])
            .unwrap()
            .is_some()
    );
    assert_eq!(store.evidence_count().unwrap(), 0);
}

#[test]
fn waiting_human_publishes_one_issue_comment_and_records_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = disposition_store(
        directory.path().join("ledger.db"),
        "WAITING_HUMAN",
        "HOLD_FOR_HUMAN",
    );
    let writer = FixtureWriter::default();

    let result = consume_disposition_once(
        (&unused_source(), &writer),
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
            target_number: 1240,
            external_id: 9001,
        }
    );
    let comments = writer.comments.borrow();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].issue_number, 1240);
    assert!(comments[0].body.contains("human decision"));
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
            (&unused_source(), &writer),
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
            (&unused_source(), &writer),
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
    disposition_store_with_event(
        path,
        state,
        effect_type,
        "FIXTURE_DISPOSITION",
        json!({"state":state}),
    )
}

#[test]
fn early_ready_takeover_posts_one_plain_language_pr_notice() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = disposition_store_with_event(
        directory.path().join("ledger.db"),
        "TAKEN_OVER",
        "RECORD_TAKEOVER",
        "HUMAN_TOOK_OVER",
        json!({"blockers":["PR_LEFT_DRAFT_STATE"]}),
    );
    let writer = FixtureWriter::default();
    let before = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert!(matches!(
        consume_disposition_once(
            (&unused_source(), &writer),
            &active_policy(),
            &mut store,
            100,
            "disposition",
            30,
            true
        )
        .unwrap(),
        DispositionCycle::Published {
            target_number: 77,
            ..
        }
    ));
    assert_eq!(store.case(&before.case_key).unwrap().unwrap(), before);
    assert_eq!(writer.comments.borrow().len(), 1);
    let body = writer.comments.borrow()[0].body.clone();
    assert!(body.contains("marked ready for review"));
    assert!(body.contains("handing it over to you"));
    assert!(body.contains("CI"));
    assert!(!body.contains("PR_LEFT_DRAFT_STATE"));
    assert!(!body.contains("repo:984321"));
    assert!(!body.contains("```"));
    assert_eq!(
        consume_disposition_once(
            (&unused_source(), &writer),
            &active_policy(),
            &mut store,
            101,
            "disposition",
            30,
            true
        )
        .unwrap(),
        DispositionCycle::Idle
    );
    assert_eq!(writer.comments.borrow().len(), 1);
}

#[test]
fn early_ready_notice_requires_authorization_and_retries_the_same_effect_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.db");
    let mut store = disposition_store_with_event(
        path.clone(),
        "TAKEN_OVER",
        "RECORD_TAKEOVER",
        "HUMAN_TOOK_OVER",
        json!({"blockers":["PR_LEFT_DRAFT_STATE"]}),
    );
    let mut writer = FixtureWriter {
        fail: true,
        ..FixtureWriter::default()
    };
    assert_eq!(
        consume_disposition_once(
            (&unused_source(), &writer),
            &active_policy(),
            &mut store,
            100,
            "disposition",
            30,
            false
        )
        .unwrap(),
        DispositionCycle::AuthorizationBlocked
    );
    assert!(writer.comments.borrow().is_empty());
    assert_eq!(store.status(100).unwrap().outbox_leased, 0);
    assert!(
        consume_disposition_once(
            (&unused_source(), &writer),
            &active_policy(),
            &mut store,
            100,
            "disposition",
            30,
            true
        )
        .is_err()
    );
    assert_eq!(store.status(100).unwrap().outbox_leased, 0);
    assert_eq!(store.evidence_count().unwrap(), 0);
    drop(store);
    let mut store = Store::open(path).unwrap();
    writer.fail = false;
    assert!(matches!(
        consume_disposition_once(
            (&unused_source(), &writer),
            &active_policy(),
            &mut store,
            101,
            "disposition",
            30,
            true
        )
        .unwrap(),
        DispositionCycle::Published { .. }
    ));
    let comments = writer.comments.borrow();
    assert_eq!(comments[0].effect_id, comments[1].effect_id);
    assert_eq!(comments[0].body, comments[1].body);
}

#[test]
fn other_takeovers_do_not_get_a_misleading_early_ready_notice() {
    for blockers in [
        json!(["FOREIGN_HEAD_COMMIT"]),
        json!(["PR_LEFT_DRAFT_STATE", "PR_DISPOSITION_CHANGED"]),
        json!(["PR_LEFT_DRAFT_STATE", "FOREIGN_PR_AUTHOR"]),
        json!(["PR_LEFT_DRAFT_STATE", "FOREIGN_HEAD_BRANCH"]),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = disposition_store_with_event(
            directory.path().join("ledger.db"),
            "TAKEN_OVER",
            "RECORD_TAKEOVER",
            "HUMAN_TOOK_OVER",
            json!({"blockers":blockers}),
        );
        let writer = FixtureWriter::default();
        assert!(matches!(
            consume_disposition_once(
                (&unused_source(), &writer),
                &active_policy(),
                &mut store,
                100,
                "disposition",
                30,
                true
            )
            .unwrap(),
            DispositionCycle::Recorded { .. }
        ));
        assert!(writer.comments.borrow().is_empty());
    }
}

fn disposition_store_with_event(
    path: std::path::PathBuf,
    state: &str,
    effect_type: &str,
    event_type: &str,
    payload: Value,
) -> Store {
    let mut store = Store::open(path).unwrap();
    let policy = active_policy();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: policy.revision,
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
                    event_type: event_type.into(),
                    payload,
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
