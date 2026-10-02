use pip_github::{
    CheckConclusion, CheckRunSnapshot, CheckStatus, CiVerdict, CommitStatusSnapshot,
    CommitStatusState, PullRequestEvidence, PullRequestSnapshot, evaluate_ci,
};

#[test]
fn unfinished_optional_checks_hold_reviews_without_hiding_failure() {
    for status in [CheckStatus::Queued, CheckStatus::InProgress] {
        let mut snapshot = evidence(
            vec![
                check(
                    1,
                    "Required CI",
                    CheckStatus::Completed,
                    Some(CheckConclusion::Success),
                ),
                check(2, "Native measurements", status, None),
            ],
            CommitStatusState::Success,
            vec![],
        );
        let required = ["Required CI".into()];
        let waiting = evaluate_ci(&snapshot, &"b".repeat(40), &required);
        assert_eq!(waiting.verdict, CiVerdict::Pending);
        assert_eq!(waiting.blockers, ["CI_PENDING"]);
        snapshot.check_runs[1].status = CheckStatus::Completed;
        snapshot.check_runs[1].conclusion = Some(CheckConclusion::Success);
        assert_eq!(
            evaluate_ci(&snapshot, &"b".repeat(40), &required).verdict,
            CiVerdict::Accepted
        );
        snapshot.check_runs[1].conclusion = Some(CheckConclusion::Failure);
        snapshot
            .check_runs
            .push(check(3, "Still running", status, None));
        assert_eq!(
            evaluate_ci(&snapshot, &"b".repeat(40), &required).verdict,
            CiVerdict::Failed
        );
    }
}

#[test]
fn the_latest_status_of_a_context_is_its_disposition() {
    for older in [CommitStatusState::Pending, CommitStatusState::Failure] {
        let evidence = evidence(
            vec![],
            CommitStatusState::Success,
            vec![
                status(1, "ci", older),
                status(2, "ci", CommitStatusState::Success),
            ],
        );
        assert_eq!(
            evaluate_ci(&evidence, &"b".repeat(40), &["ci".into()]).verdict,
            CiVerdict::Accepted
        );
    }
}

#[test]
fn exact_required_contexts_accept_only_complete_green_evidence() {
    let evidence = evidence(
        vec![check(
            1,
            "test",
            CheckStatus::Completed,
            Some(CheckConclusion::Success),
        )],
        CommitStatusState::Success,
        vec![status(2, "lint", CommitStatusState::Success)],
    );
    let evaluated = evaluate_ci(&evidence, &"b".repeat(40), &["test".into(), "lint".into()]);

    assert_eq!(evaluated.verdict, CiVerdict::Accepted);
    assert!(evaluated.blockers.is_empty());
}

#[test]
fn a_green_rerun_supersedes_an_earlier_red_attempt_of_the_same_check() {
    let evidence = evidence(
        vec![
            check(
                1,
                "test",
                CheckStatus::Completed,
                Some(CheckConclusion::Failure),
            ),
            check(
                2,
                "test",
                CheckStatus::Completed,
                Some(CheckConclusion::Success),
            ),
        ],
        CommitStatusState::Success,
        Vec::new(),
    );
    let evaluated = evaluate_ci(&evidence, &"b".repeat(40), &["test".into()]);
    assert_eq!(evaluated.verdict, CiVerdict::Accepted);

    // The other way round, the latest attempt is red and names the check.
    let mut evidence = evidence;
    evidence.check_runs[0].conclusion = Some(CheckConclusion::Success);
    evidence.check_runs[1].conclusion = Some(CheckConclusion::Failure);
    let evaluated = evaluate_ci(&evidence, &"b".repeat(40), &["test".into()]);
    assert_eq!(evaluated.verdict, CiVerdict::Failed);
    assert_eq!(
        evaluated.blockers,
        ["CHECK_FAILED:test", "REQUIRED_CONTEXT_NOT_GREEN:test"]
    );
}

#[test]
fn a_cancelled_optional_check_is_ignored_but_a_cancelled_required_one_is_not_green() {
    let evidence = evidence(
        vec![
            check(
                1,
                "Required CI",
                CheckStatus::Completed,
                Some(CheckConclusion::Success),
            ),
            check(
                2,
                "Nightly bench",
                CheckStatus::Completed,
                Some(CheckConclusion::Cancelled),
            ),
        ],
        CommitStatusState::Success,
        vec![],
    );
    assert_eq!(
        evaluate_ci(&evidence, &"b".repeat(40), &["Required CI".into()]).verdict,
        CiVerdict::Accepted
    );
    let evaluated = evaluate_ci(&evidence, &"b".repeat(40), &["Nightly bench".into()]);
    assert_eq!(evaluated.verdict, CiVerdict::Failed);
    assert_eq!(
        evaluated.blockers,
        ["REQUIRED_CONTEXT_NOT_GREEN:Nightly bench"]
    );
}

#[test]
fn pending_missing_and_hollow_evidence_never_release_reviewers() {
    let pending = evidence(
        vec![check(1, "test", CheckStatus::InProgress, None)],
        CommitStatusState::Pending,
        Vec::new(),
    );
    let evaluated = evaluate_ci(&pending, &"b".repeat(40), &["test".into(), "lint".into()]);
    assert_eq!(evaluated.verdict, CiVerdict::Pending);
    assert_eq!(
        evaluated.blockers,
        ["CI_PENDING", "MISSING_REQUIRED_CONTEXT:lint"]
    );

    let hollow = evidence(Vec::new(), CommitStatusState::Pending, Vec::new());
    let evaluated = evaluate_ci(&hollow, &"b".repeat(40), &[]);
    assert_eq!(evaluated.verdict, CiVerdict::Pending);
    assert_eq!(evaluated.blockers, ["CI_HOLLOW"]);
}

#[test]
fn exact_head_drift_and_failed_statuses_fail_closed() {
    let evidence = evidence(
        vec![check(
            1,
            "test",
            CheckStatus::Completed,
            Some(CheckConclusion::Success),
        )],
        CommitStatusState::Failure,
        vec![status(2, "lint", CommitStatusState::Failure)],
    );
    let evaluated = evaluate_ci(&evidence, &"c".repeat(40), &["test".into()]);
    assert_eq!(evaluated.verdict, CiVerdict::Failed);
    assert_eq!(
        evaluated.blockers,
        [
            "PR_HEAD_MISMATCH",
            "STATUS_FAILED:lint",
            "CHECK_HEAD_MISMATCH"
        ]
    );
}

fn evidence(
    check_runs: Vec<CheckRunSnapshot>,
    commit_status_state: CommitStatusState,
    commit_statuses: Vec<CommitStatusSnapshot>,
) -> PullRequestEvidence {
    PullRequestEvidence {
        pull_request: PullRequestSnapshot {
            id: 9,
            number: 77,
            open: true,
            draft: true,
            merged: false,
            merge_commit_sha: None,
            mergeable: Some(true),
            mergeable_state: "clean".into(),
            author_id: 10,
            head_repository_id: 984_321,
            head_repository: "owner/repo".into(),
            head_branch: "pip/issue-42".into(),
            head_sha: "b".repeat(40),
            base_branch: "master".into(),
            base_sha: "a".repeat(40),
        },
        check_runs,
        commit_status_state,
        commit_statuses,
        reviews: Vec::new(),
    }
}

fn check(
    id: u64,
    name: &str,
    status: CheckStatus,
    conclusion: Option<CheckConclusion>,
) -> CheckRunSnapshot {
    CheckRunSnapshot {
        id,
        app_id: 1,
        details_url: None,
        name: name.into(),
        head_sha: "b".repeat(40),
        status,
        conclusion,
        started_at: None,
        completed_at: None,
        check_suite_id: None,
    }
}

fn status(id: u64, context: &str, state: CommitStatusState) -> CommitStatusSnapshot {
    CommitStatusSnapshot {
        id,
        creator_id: 1,
        context: context.into(),
        state,
        created_at: "2026-08-20T00:00:00Z".into(),
        updated_at: "2026-08-20T00:01:00Z".into(),
    }
}

#[test]
fn same_named_jobs_in_different_workflows_are_judged_separately() {
    let mut failing = check(
        1,
        "test",
        CheckStatus::Completed,
        Some(CheckConclusion::Failure),
    );
    failing.check_suite_id = Some(10);
    let mut passing = check(
        2,
        "test",
        CheckStatus::Completed,
        Some(CheckConclusion::Success),
    );
    passing.check_suite_id = Some(20);
    let evaluated = evaluate_ci(
        &evidence(vec![failing, passing], CommitStatusState::Success, vec![]),
        &"b".repeat(40),
        &[],
    );
    assert_eq!(evaluated.verdict, CiVerdict::Failed);
    assert_eq!(evaluated.blockers, ["CHECK_FAILED:test"]);
}

#[test]
fn a_queued_rerun_supersedes_the_failed_attempt_it_replaces() {
    let mut failed = check(
        1,
        "test",
        CheckStatus::Completed,
        Some(CheckConclusion::Failure),
    );
    failed.started_at = Some("2026-08-20T00:00:00Z".into());
    let rerun = check(2, "test", CheckStatus::Queued, None);
    let evaluated = evaluate_ci(
        &evidence(vec![failed, rerun], CommitStatusState::Success, vec![]),
        &"b".repeat(40),
        &["test".into()],
    );
    assert_eq!(evaluated.verdict, CiVerdict::Pending);
}
