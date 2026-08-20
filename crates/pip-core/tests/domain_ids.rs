use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_core::{
    CaseId, FindingId, GitSha, IssueNumber, PlanVersion, PolicyRevision, RepositoryId,
    RepositorySlug, RunId, StateRevision, WorkflowVersion,
};

#[test]
fn case_identity_is_repository_issue_and_workflow_version() {
    let id = CaseId::new(
        RepositoryId::new(NonZeroU64::new(984_321).unwrap()),
        IssueNumber::new(NonZeroU64::new(1240).unwrap()),
        WorkflowVersion::new(NonZeroU32::new(2).unwrap()),
    );

    assert_eq!(id.to_string(), "repo:984321#1240@2");
}

#[test]
fn identifiers_reject_empty_or_noncanonical_values() {
    assert!(RepositorySlug::from_str("").is_err());
    assert!(RepositorySlug::from_str("marmot-protocol").is_err());
    assert!(RepositorySlug::from_str("marmot-protocol/mdk/extra").is_err());
    assert!(RunId::from_str(" run-1").is_err());
    assert!(FindingId::from_str("finding 1").is_err());
}

#[test]
fn git_sha_requires_exact_lowercase_hex() {
    assert!(GitSha::from_str("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").is_ok());
    assert!(GitSha::from_str("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB").is_err());
    assert!(GitSha::from_str("bbbb").is_err());
}

#[test]
fn monotonic_version_types_preserve_nonzero_values() {
    let policy = PolicyRevision::new(NonZeroU64::new(7).unwrap());
    let state = StateRevision::new(NonZeroU64::new(11).unwrap());
    let plan = PlanVersion::new(NonZeroU32::new(3).unwrap());

    assert_eq!(policy.get(), 7);
    assert_eq!(state.get(), 11);
    assert_eq!(plan.get(), 3);
}
