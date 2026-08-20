use std::cell::{Cell, RefCell};

use pip_control::{
    MergeCycle, MergeSource, MergeWriter, load_repository_policy, reconcile_merge_once,
};
use pip_github::{
    CheckConclusion, CheckRunSnapshot, CheckStatus, CommitStatusState, GitHubError,
    MergeModePolicy, MergeSpec, MutationResult, PullRequestEvidence, PullRequestReadySpec,
    PullRequestSnapshot, ReviewSnapshot, ReviewState, ReviewThreadSnapshot,
};
use pip_store::{EffectInput, EventInput, NewCase, RunInput, Store, TransitionInput};
use serde_json::{Value, json};

struct FixtureGitHub {
    evidence: RefCell<PullRequestEvidence>,
    ready_calls: Cell<u32>,
    merge_calls: Cell<u32>,
}

impl MergeSource for FixtureGitHub {
    fn pull_request(
        &self,
        _owner: &str,
        _repository: &str,
        _repository_id: u64,
        _pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError> {
        Ok(self.evidence.borrow().clone())
    }

    fn review_threads(
        &self,
        _owner: &str,
        _repository: &str,
        _repository_id: u64,
        _pull_request_number: u64,
    ) -> Result<Vec<ReviewThreadSnapshot>, GitHubError> {
        Ok(vec![ReviewThreadSnapshot {
            id: "PRRT_1".into(),
            is_resolved: true,
            is_outdated: false,
            path: "src/lib.rs".into(),
        }])
    }
}

impl MergeWriter for FixtureGitHub {
    fn mark_ready(&self, _spec: &PullRequestReadySpec) -> Result<MutationResult, GitHubError> {
        self.ready_calls.set(self.ready_calls.get() + 1);
        self.evidence.borrow_mut().pull_request.draft = false;
        Ok(MutationResult::Updated(77))
    }

    fn merge(
        &self,
        _spec: &MergeSpec,
        _policy: MergeModePolicy,
    ) -> Result<MutationResult, GitHubError> {
        self.merge_calls.set(self.merge_calls.get() + 1);
        let mut evidence = self.evidence.borrow_mut();
        evidence.pull_request.open = false;
        evidence.pull_request.merged = true;
        evidence.pull_request.merge_commit_sha = Some("d".repeat(40));
        Ok(MutationResult::Merged("d".repeat(40)))
    }
}

#[test]
fn guarded_merge_marks_ready_revalidates_and_verifies_the_merge() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ready_store(directory.path().join("ledger.db"));
    let github = fixture_github();

    assert!(matches!(
        reconcile_merge_once(
            &github,
            &github,
            &guarded_policy(),
            &mut store,
            100,
            "merger",
            30,
            true,
        )
        .unwrap(),
        MergeCycle::Prepared { .. }
    ));
    assert_eq!(github.ready_calls.get(), 1);
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "MERGING"
    );

    assert!(matches!(
        reconcile_merge_once(
            &github,
            &github,
            &guarded_policy(),
            &mut store,
            101,
            "merger",
            30,
            true,
        )
        .unwrap(),
        MergeCycle::Merged { ref merge_commit_sha, .. } if merge_commit_sha == &"d".repeat(40)
    ));
    assert_eq!(github.merge_calls.get(), 1);
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "COMPLETED"
    );
}

#[test]
fn shadow_policy_cannot_claim_or_mutate_a_merge_effect() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ready_store(directory.path().join("ledger.db"));
    let github = fixture_github();

    assert_eq!(
        reconcile_merge_once(
            &github,
            &github,
            &shadow_policy(),
            &mut store,
            100,
            "merger",
            30,
            true,
        )
        .unwrap(),
        MergeCycle::Disabled
    );
    assert_eq!(github.ready_calls.get(), 0);
    assert_eq!(github.merge_calls.get(), 0);
    assert!(
        store
            .claim_effect_matching("proof", 100, 30, &["BEGIN_MERGE"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn restart_after_external_merge_observes_success_without_merging_again() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ready_store(directory.path().join("ledger.db"));
    let github = fixture_github();
    reconcile_merge_once(
        &github,
        &github,
        &guarded_policy(),
        &mut store,
        100,
        "merger",
        30,
        true,
    )
    .unwrap();
    {
        let mut evidence = github.evidence.borrow_mut();
        evidence.pull_request.open = false;
        evidence.pull_request.merged = true;
        evidence.pull_request.merge_commit_sha = Some("d".repeat(40));
    }

    assert!(matches!(
        reconcile_merge_once(
            &github,
            &github,
            &guarded_policy(),
            &mut store,
            101,
            "merger",
            30,
            true,
        )
        .unwrap(),
        MergeCycle::Merged { .. }
    ));
    assert_eq!(github.merge_calls.get(), 0);
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "COMPLETED"
    );
}

fn ready_store(path: std::path::PathBuf) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "READY_TO_MERGE".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-ready".into(),
                event_type: "READY".into(),
                payload: json!({"fixture":true}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    for (index, result) in fixture["results"].as_array().unwrap().iter().enumerate() {
        let revision = u64::try_from(index).unwrap() + 1;
        let last = index == 4;
        store
            .apply_transition(
                &TransitionInput {
                    case_key: "repo:984321#1240@1".into(),
                    expected_revision: revision,
                    next_state: "READY_TO_MERGE".into(),
                    remediation_round: 0,
                    plan_version: 1,
                    pr_number: Some(77),
                    head_sha: Some("b".repeat(40)),
                    observed_at: revision + 1,
                    event: EventInput {
                        event_id: format!("event-fixture-{index}"),
                        event_type: "FIXTURE".into(),
                        payload: result.clone(),
                    },
                    run: Some(RunInput {
                        run_id: format!("run-fixture-{index}"),
                        task_id: result["task_id"].as_str().unwrap().into(),
                        role: result["role"].as_str().unwrap().into(),
                        payload: result.clone(),
                    }),
                    evidence: Vec::new(),
                    findings: Vec::new(),
                    effects: if last {
                        vec![EffectInput {
                            effect_id: "effect-begin-merge".into(),
                            effect_type: "BEGIN_MERGE".into(),
                            payload: json!({"case_key":"repo:984321#1240@1"}),
                        }]
                    } else {
                        Vec::new()
                    },
                },
                None,
            )
            .unwrap();
    }
    store
}

fn fixture_github() -> FixtureGitHub {
    FixtureGitHub {
        evidence: RefCell::new(PullRequestEvidence {
            pull_request: PullRequestSnapshot {
                id: 9001,
                number: 77,
                open: true,
                draft: true,
                merged: false,
                merge_commit_sha: None,
                mergeable: Some(true),
                mergeable_state: "clean".into(),
                author_id: 202_880,
                head_repository_id: 984_321,
                head_repository: "marmot-protocol/mdk".into(),
                head_branch: "pip/v2/repo-984321/issue-1240/workflow-1".into(),
                head_sha: "b".repeat(40),
                base_branch: "master".into(),
                base_sha: "a".repeat(40),
            },
            check_runs: vec![CheckRunSnapshot {
                id: 1,
                app_id: 1,
                name: "test".into(),
                head_sha: "b".repeat(40),
                status: CheckStatus::Completed,
                conclusion: Some(CheckConclusion::Success),
                started_at: None,
                completed_at: None,
            }],
            commit_status_state: CommitStatusState::Success,
            commit_statuses: Vec::new(),
            reviews: vec![
                review(81, 202_881, "reviewer-general"),
                review(82, 202_882, "reviewer-secperf"),
            ],
        }),
        ready_calls: Cell::new(0),
        merge_calls: Cell::new(0),
    }
}

fn review(id: u64, actor_id: u64, role: &str) -> ReviewSnapshot {
    ReviewSnapshot {
        id,
        actor_id,
        state: ReviewState::Approved,
        commit_id: Some("b".repeat(40)),
        exact_head: true,
        submitted_at: Some(format!("2026-08-20T00:00:{id:02}Z")),
        body: format!("Pip reviewer role: {role}"),
    }
}

fn guarded_policy() -> pip_control::RepositoryPolicy {
    policy("guarded", true)
}

fn shadow_policy() -> pip_control::RepositoryPolicy {
    policy("shadow", false)
}

fn policy(mode: &str, autonomous: bool) -> pip_control::RepositoryPolicy {
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
    value["merge"]["mode"] = json!(mode);
    value["merge"]["autonomous"] = json!(autonomous);
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}
