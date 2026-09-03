use pip_github::{
    CheckConclusion, CheckRunSnapshot, CheckStatus, CiVerdict, CommitStatusSnapshot,
    CommitStatusState, PullRequestEvidence, PullRequestSnapshot, evaluate_ci,
};

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
fn any_historical_red_attempt_is_permanently_failed_even_after_green_retry() {
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

    assert_eq!(evaluated.verdict, CiVerdict::Failed);
    assert_eq!(evaluated.blockers, ["HISTORICAL_FAILED_ATTEMPT"]);
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
fn exact_head_drift_and_failed_combined_status_fail_closed() {
    let evidence = evidence(
        vec![check(
            1,
            "test",
            CheckStatus::Completed,
            Some(CheckConclusion::Success),
        )],
        CommitStatusState::Failure,
        Vec::new(),
    );
    let evaluated = evaluate_ci(&evidence, &"c".repeat(40), &["test".into()]);
    assert_eq!(evaluated.verdict, CiVerdict::Failed);
    assert_eq!(
        evaluated.blockers,
        [
            "PR_HEAD_MISMATCH",
            "COMBINED_STATUS_FAILURE",
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
        name: name.into(),
        head_sha: "b".repeat(40),
        status,
        conclusion,
        started_at: None,
        completed_at: None,
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
