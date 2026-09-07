use std::cell::RefCell;

use pip_control::{
    BranchPublicationRequest, BranchPublisher, DraftPullRequestCycle, DraftPullRequestWriter,
    load_repository_policy, publish_draft_pull_request_once_with,
};
use pip_executor::{PublicationError, PublicationResult};
use pip_github::{GitHubError, MutationResult, PullRequestSpec};
use pip_store::{EffectInput, EventInput, NewCase, RunInput, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Default)]
struct FixtureWriter {
    specs: RefCell<Vec<PullRequestSpec>>,
    fail: bool,
}

impl DraftPullRequestWriter for FixtureWriter {
    fn ensure_draft_pull_request(
        &self,
        spec: &PullRequestSpec,
    ) -> Result<MutationResult, GitHubError> {
        self.specs.borrow_mut().push(spec.clone());
        if self.fail {
            Err(GitHubError::Transport("fixture outage".into()))
        } else {
            Ok(MutationResult::Created(77))
        }
    }
}

#[derive(Default)]
struct FixturePublisher {
    requests: RefCell<Vec<BranchPublicationRequest>>,
    remote_head: RefCell<Option<String>>,
    fail: bool,
}

impl BranchPublisher for FixturePublisher {
    fn publish_branch(
        &self,
        request: &BranchPublicationRequest,
    ) -> Result<PublicationResult, PublicationError> {
        self.requests.borrow_mut().push(request.clone());
        if self.fail {
            Err(PublicationError::RemoteRace)
        } else {
            let mut remote = self.remote_head.borrow_mut();
            if remote.as_deref() == Some(request.local_head.as_str()) {
                return Ok(PublicationResult::Existing);
            }
            if *remote != request.expected_remote_head {
                return Err(PublicationError::RemoteRace);
            }
            let result = if remote.is_some() {
                PublicationResult::Updated
            } else {
                PublicationResult::Created
            };
            *remote = Some(request.local_head.clone());
            Ok(result)
        }
    }
}

#[test]
fn controller_creates_deterministic_draft_pr_before_ci_observation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = build_store(directory.path().join("ledger.db"));
    let writer = FixtureWriter::default();
    let publisher = FixturePublisher::default();

    let result = publish_draft_pull_request_once_with(
        &writer,
        &publisher,
        &active_policy(),
        &mut store,
        100,
        "draft-pr-publisher",
        30,
        true,
    )
    .unwrap();

    assert_eq!(
        result,
        DraftPullRequestCycle::Published {
            case_key: "repo:984321#1240@1".into(),
            pull_request_number: 77,
            head_sha: "b".repeat(40),
        }
    );
    let specs = writer.specs.borrow();
    assert_eq!(
        specs[0].head_branch,
        "pip/repo-984321/issue-1240/workflow-1"
    );
    assert_eq!(specs[0].head_sha, "b".repeat(40));
    assert_eq!(specs[0].effect_id, "repo:984321#1240@1:draft-pr");
    let publications = publisher.requests.borrow();
    assert_eq!(publications.len(), 1);
    assert_eq!(
        publications[0].worktree_root,
        std::path::PathBuf::from("/var/lib/pip/worktrees/mdk")
    );
    assert_eq!(
        publications[0].worktree,
        std::path::PathBuf::from("/var/lib/pip/worktrees/mdk/repo-984321-issue-1240-workflow-1")
    );
    assert_eq!(
        publications[0].branch,
        "pip/repo-984321/issue-1240/workflow-1"
    );
    assert_eq!(
        publications[0].expected_remote_url,
        "https://github.com/marmot-protocol/mdk.git"
    );
    assert_eq!(publications[0].local_head, "b".repeat(40));
    assert_eq!(publications[0].expected_remote_head, None);
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.state, "WAITING_CI");
    assert_eq!(case.pr_number, Some(77));
    assert_eq!(case.head_sha, Some("b".repeat(40)));
    assert!(
        store
            .claim_effect_matching("ci", 100, 30, &["OBSERVE_CI"])
            .unwrap()
            .is_some()
    );
    let history = store
        .immutable_history_for_case("repo:984321#1240@1")
        .unwrap();
    let publication = history
        .evidence
        .iter()
        .find(|evidence| evidence.kind == "GITHUB_DRAFT_PULL_REQUEST_PUBLICATION")
        .unwrap();
    assert_eq!(publication.payload["branch_publication"], "created");
}

#[test]
fn outage_leaves_build_recorded_and_effect_retryable() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = build_store(directory.path().join("ledger.db"));
    let writer = FixtureWriter {
        fail: true,
        ..FixtureWriter::default()
    };
    let publisher = FixturePublisher::default();

    assert!(
        publish_draft_pull_request_once_with(
            &writer,
            &publisher,
            &active_policy(),
            &mut store,
            100,
            "draft-pr-publisher",
            30,
            true,
        )
        .is_err()
    );
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "BUILDING"
    );
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
    let retry_writer = FixtureWriter::default();
    assert!(matches!(
        publish_draft_pull_request_once_with(
            &retry_writer,
            &publisher,
            &active_policy(),
            &mut store,
            101,
            "draft-pr-retry",
            30,
            true,
        )
        .unwrap(),
        DraftPullRequestCycle::Published { .. }
    ));
    let history = store
        .immutable_history_for_case("repo:984321#1240@1")
        .unwrap();
    let publication = history
        .evidence
        .iter()
        .find(|evidence| evidence.kind == "GITHUB_DRAFT_PULL_REQUEST_PUBLICATION")
        .unwrap();
    assert_eq!(publication.payload["branch_publication"], "existing");
}

#[test]
fn branch_publication_failure_never_calls_github_and_leaves_the_effect_retryable() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = build_store(directory.path().join("ledger.db"));
    let writer = FixtureWriter::default();
    let publisher = FixturePublisher {
        fail: true,
        ..FixturePublisher::default()
    };

    assert!(
        publish_draft_pull_request_once_with(
            &writer,
            &publisher,
            &active_policy(),
            &mut store,
            100,
            "draft-pr-publisher",
            30,
            true,
        )
        .is_err()
    );
    assert!(writer.specs.borrow().is_empty());
    assert_eq!(publisher.requests.borrow().len(), 1);
    assert!(
        store
            .claim_effect_matching("retry", 100, 30, &["PUBLISH_DRAFT_PULL_REQUEST"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn publication_uses_the_accepted_event_not_a_legacy_worker_round_counter() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = remediation_store_with_round(directory.path().join("ledger.db"), 1);
    let writer = FixtureWriter::default();
    let publisher = FixturePublisher {
        remote_head: RefCell::new(Some("b".repeat(40))),
        ..FixturePublisher::default()
    };
    publish_draft_pull_request_once_with(
        &writer,
        &publisher,
        &active_policy(),
        &mut store,
        100,
        "publisher",
        30,
        true,
    )
    .unwrap();
    assert_eq!(writer.specs.borrow()[0].head_sha, "c".repeat(40));
    assert_eq!(store.runs_for_case("repo:984321#1240@1").unwrap().len(), 2);
    assert!(
        store
            .runs_for_case("repo:984321#1240@1")
            .unwrap()
            .iter()
            .all(|run| run.payload["build_round"] == 1)
    );
}

#[test]
fn publication_requires_the_current_build_recorded_event_before_external_writes() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = build_store_with_event(directory.path().join("ledger.db"), "UNRELATED_EVENT");
    let writer = FixtureWriter::default();
    let publisher = FixturePublisher::default();
    assert!(
        publish_draft_pull_request_once_with(
            &writer,
            &publisher,
            &active_policy(),
            &mut store,
            100,
            "publisher",
            30,
            true
        )
        .is_err()
    );
    assert!(writer.specs.borrow().is_empty());
    assert!(publisher.requests.borrow().is_empty());
}

#[test]
fn remediation_updates_the_same_owned_pr_to_the_new_exact_head() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = remediation_store(directory.path().join("ledger.db"));
    let writer = FixtureWriter::default();
    let publisher = FixturePublisher {
        remote_head: RefCell::new(Some("b".repeat(40))),
        ..FixturePublisher::default()
    };

    publish_draft_pull_request_once_with(
        &writer,
        &publisher,
        &active_policy(),
        &mut store,
        100,
        "draft-pr-publisher",
        30,
        true,
    )
    .unwrap();

    let spec = &writer.specs.borrow()[0];
    assert_eq!(spec.effect_id, "repo:984321#1240@1:draft-pr");
    assert_eq!(spec.head_sha, "c".repeat(40));
    let publications = publisher.requests.borrow();
    assert_eq!(publications[0].local_head, "c".repeat(40));
    assert_eq!(publications[0].expected_remote_head, Some("b".repeat(40)));
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.pr_number, Some(77));
    assert_eq!(case.head_sha, Some("c".repeat(40)));
    assert_eq!(case.state, "WAITING_CI");
}

fn build_store(path: std::path::PathBuf) -> Store {
    build_store_with_event(path, "BUILD_RECORDED")
}

fn build_store_with_event(path: std::path::PathBuf, event_type: &str) -> Store {
    let result = builder_fixture();
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: active_policy().revision,
            initial_state: "BUILDING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-builder-active".into(),
                event_type: "BUILDER_DISPATCHED".into(),
                payload: json!({"fixture":true}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 1,
                next_state: "BUILDING".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 2,
                event: EventInput {
                    event_id: "event-build-recorded".into(),
                    event_type: event_type.into(),
                    payload: result.clone(),
                },
                run: Some(RunInput {
                    run_id: "run-builder-1".into(),
                    task_id: "builder-1".into(),
                    role: "builder".into(),
                    payload: result,
                }),
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-publish-draft-pr".into(),
                    effect_type: "PUBLISH_DRAFT_PULL_REQUEST".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn remediation_store(path: std::path::PathBuf) -> Store {
    remediation_store_with_round(path, 2)
}

fn remediation_store_with_round(path: std::path::PathBuf, reported_round: u32) -> Store {
    let mut result = builder_fixture();
    result["task_id"] = json!("builder-2");
    result["build_round"] = json!(reported_round);
    result["head_sha"] = json!("c".repeat(40));
    let mut store = build_store(path);
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 2,
                next_state: "REMEDIATING".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 3,
                event: EventInput {
                    event_id: "event-build-recorded-2".into(),
                    event_type: "BUILD_RECORDED".into(),
                    payload: result.clone(),
                },
                run: Some(RunInput {
                    run_id: "run-builder-2".into(),
                    task_id: "builder-2".into(),
                    role: "builder".into(),
                    payload: result,
                }),
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-publish-draft-pr-2".into(),
                    effect_type: "PUBLISH_DRAFT_PULL_REQUEST".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn builder_fixture() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    fixture["results"][1].clone()
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
