use std::cell::RefCell;

use pip_control::{
    PullRequestSource, TakeoverCycle, load_repository_policy, reconcile_takeover_once,
};
use pip_github::{CommitStatusState, GitHubError, PullRequestEvidence, PullRequestSnapshot};
use pip_store::{EffectInput, EventInput, EvidenceInput, NewCase, Store, TransitionInput};
use serde_json::{Value, json};

struct FakePullRequest {
    evidence: RefCell<PullRequestEvidence>,
}

impl PullRequestSource for FakePullRequest {
    fn pull_request(
        &self,
        _owner: &str,
        _repository: &str,
        _repository_id: u64,
        _pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError> {
        Ok(self.evidence.borrow().clone())
    }
}

#[test]
fn exact_controller_owned_pull_request_remains_automated() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    seed_bound_case(&mut store, "REVIEWING");
    let policy = active_policy();
    let source = FakePullRequest {
        evidence: RefCell::new(pull_request()),
    };

    assert_eq!(
        reconcile_takeover_once(&source, &policy, &mut store, 100).unwrap(),
        TakeoverCycle::Owned { case_count: 1 }
    );
    assert_eq!(store.case(case_key()).unwrap().unwrap().state, "REVIEWING");
}

#[test]
fn only_completed_review_allows_the_owned_pr_to_leave_draft() {
    for (state, head_changed, expected) in [
        ("SHADOW_READY", false, "SHADOW_READY"),
        ("SHADOW_READY", true, "TAKEN_OVER"),
        ("FINAL_REVIEW", false, "TAKEN_OVER"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        seed_bound_case(&mut store, state);
        let mut evidence = pull_request();
        evidence.pull_request.draft = false;
        if head_changed {
            evidence.pull_request.head_sha = "c".repeat(40);
        }
        let source = FakePullRequest {
            evidence: RefCell::new(evidence),
        };
        reconcile_takeover_once(&source, &active_policy(), &mut store, 100).unwrap();
        assert_eq!(store.case(case_key()).unwrap().unwrap().state, expected);
    }
}

#[test]
fn foreign_actor_or_protected_head_change_commits_takeover_and_supersedes_work() {
    for mutate in [
        |pull: &mut PullRequestEvidence| pull.pull_request.author_id = 999,
        |pull: &mut PullRequestEvidence| pull.pull_request.head_sha = "c".repeat(40),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        seed_bound_case(&mut store, "REVIEWING");
        let policy = active_policy();
        let mut evidence = pull_request();
        mutate(&mut evidence);
        let source = FakePullRequest {
            evidence: RefCell::new(evidence),
        };

        let TakeoverCycle::Transitioned { cases } =
            reconcile_takeover_once(&source, &policy, &mut store, 100).unwrap()
        else {
            panic!("foreign ownership must transition")
        };
        assert_eq!(cases, [case_key()]);
        assert_eq!(store.case(case_key()).unwrap().unwrap().state, "TAKEN_OVER");
        let status = store.status(101).unwrap();
        assert_eq!(status.outbox_superseded, 1);
        assert_eq!(status.outbox_pending, 1);
        assert_eq!(
            store
                .claim_effect("controller", 101, 30)
                .unwrap()
                .unwrap()
                .effect_type,
            "RECORD_TAKEOVER"
        );
    }
}

#[test]
fn remediation_may_advance_the_owned_branch_but_cannot_change_its_owner_or_name() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    seed_bound_case(&mut store, "REMEDIATING");
    let policy = active_policy();
    let mut evidence = pull_request();
    evidence.pull_request.head_sha = "c".repeat(40);
    let source = FakePullRequest {
        evidence: RefCell::new(evidence),
    };

    assert_eq!(
        reconcile_takeover_once(&source, &policy, &mut store, 100).unwrap(),
        TakeoverCycle::Owned { case_count: 1 }
    );
    assert_eq!(
        store.case(case_key()).unwrap().unwrap().state,
        "REMEDIATING"
    );
}

#[test]
fn merged_ready_pr_completes_once_without_publishing_or_dispatching_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    seed_bound_case(&mut store, "SHADOW_READY");
    let source = FakePullRequest {
        evidence: RefCell::new(merged_pull()),
    };
    reconcile_takeover_once(&source, &active_policy(), &mut store, 100).unwrap();
    assert_eq!(store.case(case_key()).unwrap().unwrap().state, "COMPLETED");
    let history = store.immutable_history_for_case(case_key()).unwrap();
    assert_eq!(history.events.last().unwrap().event_type, "HUMAN_MERGED");
    assert_eq!(
        history.events.last().unwrap().payload["merge_commit_sha"],
        "d".repeat(40)
    );
    assert_eq!(
        store
            .claim_effect("local", 101, 30)
            .unwrap()
            .unwrap()
            .effect_type,
        "RECORD_COMPLETION"
    );
    let before = store.status(102).unwrap();
    assert_eq!(
        reconcile_takeover_once(&source, &active_policy(), &mut store, 102).unwrap(),
        TakeoverCycle::Idle
    );
    assert_eq!(store.status(102).unwrap(), before);
}

#[test]
fn closure_early_merge_and_foreign_or_unverified_merges_are_not_success() {
    for fault in [
        "early",
        "closed",
        "head",
        "author",
        "branch",
        "missing-merge-sha",
        "bad-merge-sha",
        "still-open",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        seed_bound_case(
            &mut store,
            if fault == "early" {
                "FINAL_REVIEW"
            } else {
                "SHADOW_READY"
            },
        );
        let mut evidence = merged_pull();
        match fault {
            "closed" => evidence.pull_request.merged = false,
            "head" => evidence.pull_request.head_sha = "c".repeat(40),
            "author" => evidence.pull_request.author_id = 999,
            "branch" => evidence.pull_request.head_branch = "someone-else".into(),
            "missing-merge-sha" => evidence.pull_request.merge_commit_sha = None,
            "bad-merge-sha" => evidence.pull_request.merge_commit_sha = Some("invalid".into()),
            "still-open" => evidence.pull_request.open = true,
            _ => {}
        }
        let source = FakePullRequest {
            evidence: RefCell::new(evidence),
        };
        reconcile_takeover_once(&source, &active_policy(), &mut store, 100).unwrap();
        assert_eq!(
            store.case(case_key()).unwrap().unwrap().state,
            "TAKEN_OVER",
            "{fault}"
        );
    }
}

fn merged_pull() -> PullRequestEvidence {
    let mut evidence = pull_request();
    evidence.pull_request.open = false;
    evidence.pull_request.draft = false;
    evidence.pull_request.merged = true;
    evidence.pull_request.merge_commit_sha = Some("d".repeat(40));
    evidence
}

#[test]
fn legacy_merge_classification_is_corrected_without_rewriting_history_or_real_takeovers() {
    for fault in [
        "none",
        "early",
        "extra-blocker",
        "not-merged-then",
        "not-merged-now",
        "head-changed",
        "base-changed",
        "missing-evidence",
        "merge-changed",
        "old-head",
        "old-author",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        seed_bound_case(
            &mut store,
            if fault == "early" {
                "FINAL_REVIEW"
            } else {
                "SHADOW_READY"
            },
        );
        let mut old_pull = merged_pull();
        if fault == "not-merged-then" {
            old_pull.pull_request.merged = false;
        }
        if fault == "old-head" {
            old_pull.pull_request.head_sha = "e".repeat(40);
        }
        if fault == "old-author" {
            old_pull.pull_request.author_id = 999;
        }
        let blockers = if fault == "extra-blocker" {
            vec!["PR_DISPOSITION_CHANGED", "FOREIGN_HEAD_COMMIT"]
        } else {
            vec!["PR_DISPOSITION_CHANGED"]
        };
        let evidence = if fault == "missing-evidence" {
            vec![]
        } else {
            vec![EvidenceInput {
                evidence_id: "evidence-takeover-repo1055628515-issue1240-workflow2-revision2"
                    .into(),
                kind: "GITHUB_TAKEOVER".into(),
                source: "github-pr-77".into(),
                payload: serde_json::to_value(old_pull).unwrap(),
            }]
        };
        store
            .apply_transition(
                &TransitionInput {
                    case_key: case_key().into(),
                    expected_revision: 2,
                    next_state: "TAKEN_OVER".into(),
                    remediation_round: 0,
                    plan_version: 1,
                    pr_number: Some(77),
                    head_sha: Some("b".repeat(40)),
                    observed_at: 90,
                    event: EventInput {
                        event_id: "legacy-takeover".into(),
                        event_type: "HUMAN_TOOK_OVER".into(),
                        payload: json!({"blockers": blockers}),
                    },
                    run: None,
                    findings: vec![],
                    effects: vec![],
                    evidence,
                },
                None,
            )
            .unwrap();
        let before = store.immutable_history_for_case(case_key()).unwrap();
        let mut fresh = merged_pull();
        if fault == "not-merged-now" {
            fresh.pull_request.merged = false;
        }
        if fault == "head-changed" {
            fresh.pull_request.head_sha = "e".repeat(40);
        }
        if fault == "merge-changed" {
            fresh.pull_request.merge_commit_sha = Some("e".repeat(40));
        }
        if fault == "base-changed" {
            fresh.pull_request.base_branch = "other".into();
        }
        let source = FakePullRequest {
            evidence: RefCell::new(fresh),
        };
        let _ = reconcile_takeover_once(&source, &active_policy(), &mut store, 100);
        let after = store.immutable_history_for_case(case_key()).unwrap();
        assert_eq!(
            &after.events[..before.events.len()],
            before.events.as_slice()
        );
        assert_eq!(
            &after.evidence[..before.evidence.len()],
            before.evidence.as_slice()
        );
        assert_eq!(
            store.case(case_key()).unwrap().unwrap().state,
            if fault == "none" {
                "COMPLETED"
            } else {
                "TAKEN_OVER"
            },
            "{fault}"
        );
        if fault == "none" {
            assert_eq!(after.events.last().unwrap().event_type, "HUMAN_MERGED");
        } else {
            assert_eq!(after, before, "{fault}");
        }
    }
}

fn active_policy() -> pip_control::RepositoryPolicy {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["intake"]["enabled"] = json!(true);
    value["intake"]["paused"] = json!(false);
    value["dispatch_enabled"] = json!(true);
    value["github"]["automation_actor_id"] = json!(202880);
    value["github"]["reviewer_general_actor_id"] = json!(202881);
    value["github"]["reviewer_secperf_actor_id"] = json!(202882);
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn case_key() -> &'static str {
    "repo:1055628515#1240@2"
}

fn seed_bound_case(store: &mut Store, state: &str) {
    store
        .create_case(&NewCase {
            case_key: case_key().into(),
            repository_id: 1_055_628_515,
            issue_number: 1240,
            workflow_version: 2,
            policy_revision: active_policy().revision,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: case_key().into(),
                expected_revision: 1,
                next_state: state.into(),
                remediation_round: u32::from(state == "REMEDIATING"),
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 2,
                event: EventInput {
                    event_id: "event-bound".into(),
                    event_type: "TEST_BOUND".into(),
                    payload: json!({}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-pending".into(),
                    effect_type: if state == "REMEDIATING" {
                        "DISPATCH_BUILDER"
                    } else {
                        "DISPATCH_REVIEWERS"
                    }
                    .into(),
                    payload: json!({}),
                }],
            },
            None,
        )
        .unwrap();
}

fn pull_request() -> PullRequestEvidence {
    PullRequestEvidence {
        pull_request: PullRequestSnapshot {
            id: 700,
            number: 77,
            open: true,
            draft: true,
            merged: false,
            merge_commit_sha: None,
            mergeable: Some(true),
            mergeable_state: "clean".into(),
            author_id: 202880,
            head_repository_id: 1_055_628_515,
            head_repository: "marmot-protocol/mdk".into(),
            head_branch: "pip/repo-1055628515/issue-1240/workflow-2".into(),
            head_sha: "b".repeat(40),
            base_branch: "master".into(),
            base_sha: "a".repeat(40),
        },
        check_runs: Vec::new(),
        commit_status_state: CommitStatusState::Pending,
        commit_statuses: Vec::new(),
        reviews: Vec::new(),
    }
}
