use std::cell::RefCell;

use pip_control::{
    BranchPublication, BranchPublicationRequest, BranchPublisher, DraftPullRequestCycle,
    DraftPullRequestWriter, load_repository_policy, publish_draft_pull_request_once,
    publish_draft_pull_request_once_with,
};
use pip_executor::{PublicationError, PublicationResult, SignedCommit};
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
    signed: Option<SignedCommit>,
}

impl BranchPublisher for FixturePublisher {
    fn publish_branch(
        &self,
        request: &BranchPublicationRequest,
    ) -> Result<BranchPublication, PublicationError> {
        self.requests.borrow_mut().push(request.clone());
        if self.fail {
            Err(PublicationError::RemoteRace)
        } else {
            let mut remote = self.remote_head.borrow_mut();
            let head = self
                .signed
                .as_ref()
                .map(|signed| signed.head.to_string())
                .unwrap_or_else(|| request.local_head.clone());
            if remote.as_deref() == Some(head.as_str()) {
                return Ok(BranchPublication {
                    result: PublicationResult::Existing,
                    signed: self.signed.clone(),
                });
            }
            if *remote != request.expected_remote_head {
                return Err(PublicationError::RemoteRace);
            }
            let result = if remote.is_some() {
                PublicationResult::Updated
            } else {
                PublicationResult::Created
            };
            *remote = Some(head);
            Ok(BranchPublication {
                result,
                signed: self.signed.clone(),
            })
        }
    }
}

#[test]
fn signed_republication_reuses_the_accepted_tree_and_plan_without_rewriting_history() {
    for (remediated, legacy_signed) in [(false, false), (true, false), (true, true)] {
        let temp = tempfile::tempdir().unwrap();
        let policy = active_policy();
        let mut store = if remediated {
            remediation_store_with_head(temp.path().join("ledger.db"), 2, &"f".repeat(40))
        } else {
            build_store(temp.path().join("ledger.db"))
        };
        store
            .record_policy(&pip_store::PolicyInput {
                repository_id: policy.repository.id,
                revision: policy.revision,
                accepted_at: 1,
                payload: serde_json::to_value(&policy).unwrap(),
            })
            .unwrap();
        let writer = FixtureWriter::default();
        publish_draft_pull_request_once_with(
            &writer,
            &FixturePublisher {
                remote_head: RefCell::new(remediated.then(|| "b".repeat(40))),
                signed: legacy_signed.then(|| SignedCommit {
                    source_head: "f".repeat(40).parse().unwrap(),
                    head: "9".repeat(40).parse().unwrap(),
                    parent: "b".repeat(40).parse().unwrap(),
                    tree: "e".repeat(40).parse().unwrap(),
                    signer_fingerprint: "SHA256:fixture".into(),
                    integrated_base: None,
                }),
                ..FixturePublisher::default()
            },
            &policy,
            &mut store,
            100,
            "publisher",
            30,
            true,
        )
        .unwrap();
        let case = store.case("repo:984321#1240@1").unwrap().unwrap();
        let before = store.immutable_history_for_case(&case.case_key).unwrap();
        let mut paused = policy.clone();
        paused.intake.enabled = false;
        paused.intake.paused = true;
        paused.dispatch_enabled = false;
        let request = pip_control::PublicationRetryRequest {
            case_key: case.case_key.clone(),
            expected_revision: case.state_revision,
            expected_head: case.head_sha.clone().unwrap(),
            request_id: "event-sign-publication".into(),
            reason: "Replace unsigned publication with controller-signed accepted tree".into(),
        };
        for (invalid, uid, active) in [
            (request.clone(), 1000, false),
            (request.clone(), 0, true),
            (request.clone(), 0, false),
            (request.clone(), 0, false),
            (request.clone(), 0, false),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (mut req, uid, active))| {
            match index {
                2 => req.expected_revision += 1,
                3 => req.expected_head = "a".repeat(40),
                4 => req.reason.clear(),
                _ => (),
            }
            (req, uid, active)
        }) {
            assert!(
                pip_control::authorize_publication_retry(
                    &mut store,
                    if active { &policy } else { &paused },
                    &invalid,
                    101,
                    uid
                )
                .is_err()
            );
            assert_eq!(
                store.immutable_history_for_case(&case.case_key).unwrap(),
                before
            );
        }
        assert_eq!(
            pip_control::authorize_publication_retry(&mut store, &paused, &request, 101, 0)
                .unwrap(),
            pip_store::ApplyResult::Applied
        );
        let prepared = store.case(&case.case_key).unwrap().unwrap();
        assert_eq!(prepared.state, "BUILDING");
        assert_eq!(prepared.plan_version, case.plan_version);
        assert_eq!(prepared.remediation_round, case.remediation_round);
        assert_eq!(prepared.head_sha, case.head_sha);
        let unsigned = FixturePublisher {
            remote_head: RefCell::new(case.head_sha.clone()),
            ..FixturePublisher::default()
        };
        assert!(
            publish_draft_pull_request_once_with(
                &writer,
                &unsigned,
                &policy,
                &mut store,
                102,
                "publisher",
                30,
                true
            )
            .is_err()
        );
        assert_eq!(
            store.case(&case.case_key).unwrap().unwrap().state,
            "BUILDING"
        );
        assert_eq!(store.status(102).unwrap().outbox_leased, 0);
        let publisher = FixturePublisher {
            remote_head: RefCell::new(case.head_sha.clone()),
            signed: Some(SignedCommit {
                source_head: (if remediated { "f" } else { "b" })
                    .repeat(40)
                    .parse()
                    .unwrap(),
                head: "d".repeat(40).parse().unwrap(),
                parent: "c".repeat(40).parse().unwrap(),
                tree: "e".repeat(40).parse().unwrap(),
                signer_fingerprint: "SHA256:fixture".into(),
                integrated_base: Some("c".repeat(40).parse().unwrap()),
            }),
            ..FixturePublisher::default()
        };
        publish_draft_pull_request_once_with(
            &writer,
            &publisher,
            &policy,
            &mut store,
            102,
            "publisher",
            30,
            true,
        )
        .unwrap();
        assert_eq!(publisher.requests.borrow()[0].parent_head, "c".repeat(40));
        let after = store.immutable_history_for_case(&case.case_key).unwrap();
        assert_eq!(after.runs, before.runs);
        assert_eq!(
            &after.events[..before.events.len()],
            before.events.as_slice()
        );
        assert_eq!(
            store.case(&case.case_key).unwrap().unwrap().state,
            "WAITING_CI"
        );
        assert_eq!(
            store.case(&case.case_key).unwrap().unwrap().head_sha,
            Some("d".repeat(40))
        );
        assert_eq!(
            pip_control::authorize_publication_retry(&mut store, &paused, &request, 103, 0)
                .unwrap(),
            pip_store::ApplyResult::Replayed
        );
        assert_eq!(
            store.immutable_history_for_case(&case.case_key).unwrap(),
            after
        );
        let mut conflicting = request.clone();
        conflicting.reason = "Different request".into();
        assert!(
            pip_control::authorize_publication_retry(&mut store, &paused, &conflicting, 103, 0)
                .is_err()
        );
        let current = store.case(&case.case_key).unwrap().unwrap();
        let signed_retry = pip_control::PublicationRetryRequest {
            expected_revision: current.state_revision,
            expected_head: current.head_sha.unwrap(),
            request_id: "event-sign-again".into(),
            ..request
        };
        assert!(
            pip_control::authorize_publication_retry(&mut store, &paused, &signed_retry, 103, 0)
                .is_err()
        );
        assert_eq!(
            store.immutable_history_for_case(&case.case_key).unwrap(),
            after
        );
    }
}

#[test]
fn missing_signing_credentials_only_block_pending_publication() {
    let temp = tempfile::tempdir().unwrap();
    let mut empty = Store::open(temp.path().join("empty.db")).unwrap();
    let writer = FixtureWriter::default();
    let missing = std::path::Path::new("/missing/pip-publication-credential");
    assert_eq!(
        publish_draft_pull_request_once(
            &writer,
            &active_policy(),
            &mut empty,
            missing,
            missing,
            None,
            100,
            "publisher",
            30,
            true
        )
        .unwrap(),
        DraftPullRequestCycle::Idle
    );
    let mut pending = build_store(temp.path().join("pending.db"));
    let error = publish_draft_pull_request_once(
        &writer,
        &active_policy(),
        &mut pending,
        missing,
        missing,
        None,
        100,
        "publisher",
        30,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("signing credentials"));
    assert!(writer.specs.borrow().is_empty());
    assert_eq!(pending.status(100).unwrap().outbox_leased, 0);
    assert_eq!(pending.status(100).unwrap().outbox_pending, 1);
}

#[test]
fn signed_publication_records_the_mapping_without_rewriting_the_builder_result() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = build_store(directory.path().join("ledger.db"));
    let before = store.runs_for_case("repo:984321#1240@1").unwrap();
    let publisher = FixturePublisher {
        signed: Some(SignedCommit {
            source_head: "b".repeat(40).parse().unwrap(),
            head: "d".repeat(40).parse().unwrap(),
            tree: "e".repeat(40).parse().unwrap(),
            parent: "c".repeat(40).parse().unwrap(),
            signer_fingerprint: "SHA256:fixture".into(),
            integrated_base: None,
        }),
        ..FixturePublisher::default()
    };
    let writer = FixtureWriter::default();
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
    assert_eq!(writer.specs.borrow()[0].head_sha, "d".repeat(40));
    assert!(writer.specs.borrow()[0].body.contains(&"d".repeat(40)));
    assert!(!writer.specs.borrow()[0].body.contains(&"b".repeat(40)));
    assert_eq!(store.runs_for_case("repo:984321#1240@1").unwrap(), before);
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.head_sha, Some("d".repeat(40)));
    assert_eq!(case.state, "WAITING_CI");
    let history = store.immutable_history_for_case(&case.case_key).unwrap();
    let publication = history
        .evidence
        .iter()
        .find(|e| e.kind == "GITHUB_DRAFT_PULL_REQUEST_PUBLICATION")
        .unwrap();
    assert_eq!(
        publication.payload["signing"]["source_head"],
        "b".repeat(40)
    );
    assert_eq!(publication.payload["signing"]["head"], "d".repeat(40));
    assert_eq!(publication.payload["signing"]["parent"], "c".repeat(40));
    assert_eq!(publication.payload["signing"]["tree"], "e".repeat(40));
}

#[test]
fn invalid_signing_bindings_never_reach_the_pr_writer() {
    for (source, parent, head, fingerprint) in [
        ("a", "c", "d", "SHA256:fixture"),
        ("b", "a", "d", "SHA256:fixture"),
        ("b", "c", "b", "SHA256:fixture"),
        ("b", "c", "d", "SHA256:"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut store = build_store(temp.path().join("ledger.db"));
        let writer = FixtureWriter::default();
        let publisher = FixturePublisher {
            signed: Some(SignedCommit {
                source_head: source.repeat(40).parse().unwrap(),
                head: head.repeat(40).parse().unwrap(),
                tree: "e".repeat(40).parse().unwrap(),
                parent: parent.repeat(40).parse().unwrap(),
                signer_fingerprint: fingerprint.into(),
                integrated_base: None,
            }),
            ..FixturePublisher::default()
        };
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
        assert_eq!(store.status(100).unwrap().outbox_leased, 0);
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
    assert!(!specs[0].body.contains("```json"));
    assert!(specs[0].body.contains("### Local checks"));
    assert!(specs[0].body.contains("No findings required remediation."));
    assert_eq!(specs[0].title, "Bound retained profile metadata");
    for expected in [
        "Fixes #1240",
        "### Problem",
        "Unknown fields bypass retention limits.",
        "### Solution",
        "Bound keys and values during profile ingestion.",
        "https://github.com/marmot-protocol/mdk/issues/1240#issuecomment-991",
    ] {
        assert!(specs[0].body.contains(expected), "missing {expected}");
    }
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
    assert_eq!(
        store
            .runs_for_case("repo:984321#1240@1")
            .unwrap()
            .iter()
            .filter(|run| run.role == "builder")
            .count(),
        2
    );
    assert!(
        store
            .runs_for_case("repo:984321#1240@1")
            .unwrap()
            .iter()
            .filter(|run| run.role == "builder")
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
    assert_eq!(
        publications[0].target_branch,
        active_policy().repository.default_branch
    );
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
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    let plan = fixture["results"][0].clone();
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
                    event_id: "event-plan-recorded".into(),
                    event_type: "PLAN_RECORDED".into(),
                    payload: plan.clone(),
                },
                run: Some(RunInput {
                    run_id: "run-planner-1".into(),
                    task_id: plan["task_id"].as_str().unwrap().into(),
                    role: "planner".into(),
                    payload: plan.clone(),
                }),
                evidence: vec![pip_store::EvidenceInput {
                    evidence_id: "published-plan".into(),
                    kind: "GITHUB_PLAN_PUBLICATION".into(),
                    source: "github-issue-1240".into(),
                    payload: json!({
                        "actor_id": 202880,
                        "comment_id": 991,
                        "plan_version": 1,
                        "task_id": plan["task_id"],
                    }),
                }],
                findings: Vec::new(),
                effects: Vec::new(),
            },
            None,
        )
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 2,
                next_state: "BUILDING".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 3,
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
    remediation_store_with_head(path, reported_round, &"c".repeat(40))
}

fn remediation_store_with_head(path: std::path::PathBuf, reported_round: u32, head: &str) -> Store {
    let mut result = builder_fixture();
    result["task_id"] = json!("builder-2");
    result["build_round"] = json!(reported_round);
    result["head_sha"] = json!(head);
    let mut store = build_store(path);
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 3,
                next_state: "REMEDIATING".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 4,
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
    let mut result = fixture["results"][1].clone();
    result["evidence"]["pr_title"] = json!("Bound retained profile metadata");
    result["evidence"]["problem_summary"] = json!("Unknown fields bypass retention limits.");
    result["evidence"]["solution_summary"] =
        json!("Bound keys and values during profile ingestion.");
    result
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
