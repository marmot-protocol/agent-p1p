use std::fmt;
use std::str::FromStr;

use crate::{CaseId, FindingId, GitSha, PlanVersion, PullRequestNumber};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReviewRole {
    General,
    SecurityPerformance,
}

impl FromStr for ReviewRole {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "reviewer-general" => Ok(Self::General),
            "reviewer-secperf" => Ok(Self::SecurityPerformance),
            _ => Err("unknown mandatory review role"),
        }
    }
}

impl fmt::Display for ReviewRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::General => "reviewer-general",
            Self::SecurityPerformance => "reviewer-secperf",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadBinding {
    pub case_id: CaseId,
    pub pr_number: PullRequestNumber,
    pub plan_version: PlanVersion,
    pub head_sha: GitSha,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuilderEvidence {
    pub binding: HeadBinding,
    pub ci_head_sha: GitSha,
    pub ci_green: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CiEvidence {
    pub head_sha: GitSha,
    pub completed: bool,
    pub hollow: bool,
    pub rate_limited: bool,
    pub required_checks_green: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewEvidence {
    pub binding: HeadBinding,
    pub role: ReviewRole,
    pub approved: bool,
    pub blocking_findings: Vec<FindingId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindingEvidence {
    pub id: FindingId,
    pub origin: ReviewRole,
    pub reviewed_head_sha: GitSha,
    pub resolution_head_sha: Option<GitSha>,
    pub mandatory: bool,
    pub origin_confirmed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactHeadObservation {
    pub expected: HeadBinding,
    pub builder: BuilderEvidence,
    pub ci: CiEvidence,
    pub general_review: ReviewEvidence,
    pub secperf_review: ReviewEvidence,
    pub findings: Vec<FindingEvidence>,
    pub open_blocking_threads: u32,
    pub pip_owned: bool,
    pub mergeable: bool,
    pub authorization_valid: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JoinBlocker {
    BuilderBindingMismatch,
    BuilderHeadMismatch,
    BuilderCiHeadMismatch,
    BuilderCiNotGreen,
    CiHeadMismatch,
    CiIncomplete,
    CiHollow,
    CiRateLimited,
    CiNotGreen,
    GeneralReviewBindingMismatch,
    GeneralReviewRoleMismatch,
    GeneralReviewHeadMismatch,
    GeneralReviewNotApproved,
    GeneralReviewHasBlockingFindings,
    SecperfReviewBindingMismatch,
    SecperfReviewRoleMismatch,
    SecperfReviewHeadMismatch,
    SecperfReviewNotApproved,
    SecperfReviewHasBlockingFindings,
    FindingResolutionMissing,
    FindingResolutionHeadMismatch,
    FindingNotOriginConfirmed,
    BlockingThreadsOpen,
    ForeignOwnership,
    NotMergeable,
    AuthorizationInvalid,
}

impl fmt::Display for JoinBlocker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BuilderBindingMismatch => "BUILDER_BINDING_MISMATCH",
            Self::BuilderHeadMismatch => "BUILDER_HEAD_MISMATCH",
            Self::BuilderCiHeadMismatch => "BUILDER_CI_HEAD_MISMATCH",
            Self::BuilderCiNotGreen => "BUILDER_CI_NOT_GREEN",
            Self::CiHeadMismatch => "CI_HEAD_MISMATCH",
            Self::CiIncomplete => "CI_INCOMPLETE",
            Self::CiHollow => "CI_HOLLOW",
            Self::CiRateLimited => "CI_RATE_LIMITED",
            Self::CiNotGreen => "CI_NOT_GREEN",
            Self::GeneralReviewBindingMismatch => "GENERAL_REVIEW_BINDING_MISMATCH",
            Self::GeneralReviewRoleMismatch => "GENERAL_REVIEW_ROLE_MISMATCH",
            Self::GeneralReviewHeadMismatch => "GENERAL_REVIEW_HEAD_MISMATCH",
            Self::GeneralReviewNotApproved => "GENERAL_REVIEW_NOT_APPROVED",
            Self::GeneralReviewHasBlockingFindings => "GENERAL_REVIEW_HAS_BLOCKING_FINDINGS",
            Self::SecperfReviewBindingMismatch => "SECPERF_REVIEW_BINDING_MISMATCH",
            Self::SecperfReviewRoleMismatch => "SECPERF_REVIEW_ROLE_MISMATCH",
            Self::SecperfReviewHeadMismatch => "SECPERF_REVIEW_HEAD_MISMATCH",
            Self::SecperfReviewNotApproved => "SECPERF_REVIEW_NOT_APPROVED",
            Self::SecperfReviewHasBlockingFindings => "SECPERF_REVIEW_HAS_BLOCKING_FINDINGS",
            Self::FindingResolutionMissing => "FINDING_RESOLUTION_MISSING",
            Self::FindingResolutionHeadMismatch => "FINDING_RESOLUTION_HEAD_MISMATCH",
            Self::FindingNotOriginConfirmed => "FINDING_NOT_ORIGIN_CONFIRMED",
            Self::BlockingThreadsOpen => "BLOCKING_THREADS_OPEN",
            Self::ForeignOwnership => "FOREIGN_OWNERSHIP",
            Self::NotMergeable => "NOT_MERGEABLE",
            Self::AuthorizationInvalid => "AUTHORIZATION_INVALID",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExactHeadDecision {
    pub ready: bool,
    pub blockers: Vec<JoinBlocker>,
}

fn same_identity(left: &HeadBinding, right: &HeadBinding) -> bool {
    left.case_id == right.case_id
        && left.pr_number == right.pr_number
        && left.plan_version == right.plan_version
}

fn check_review(
    expected: &HeadBinding,
    evidence: &ReviewEvidence,
    role: ReviewRole,
    blockers: &mut Vec<JoinBlocker>,
) {
    let (binding, role_mismatch, head, approval, findings) = match role {
        ReviewRole::General => (
            JoinBlocker::GeneralReviewBindingMismatch,
            JoinBlocker::GeneralReviewRoleMismatch,
            JoinBlocker::GeneralReviewHeadMismatch,
            JoinBlocker::GeneralReviewNotApproved,
            JoinBlocker::GeneralReviewHasBlockingFindings,
        ),
        ReviewRole::SecurityPerformance => (
            JoinBlocker::SecperfReviewBindingMismatch,
            JoinBlocker::SecperfReviewRoleMismatch,
            JoinBlocker::SecperfReviewHeadMismatch,
            JoinBlocker::SecperfReviewNotApproved,
            JoinBlocker::SecperfReviewHasBlockingFindings,
        ),
    };
    if !same_identity(expected, &evidence.binding) {
        blockers.push(binding);
    }
    if evidence.role != role {
        blockers.push(role_mismatch);
    }
    if evidence.binding.head_sha != expected.head_sha {
        blockers.push(head);
    }
    if !evidence.approved {
        blockers.push(approval);
    }
    if !evidence.blocking_findings.is_empty() {
        blockers.push(findings);
    }
}

#[must_use]
pub fn evaluate_exact_head(observation: &ExactHeadObservation) -> ExactHeadDecision {
    let mut blockers = Vec::new();
    let expected = &observation.expected;
    if !same_identity(expected, &observation.builder.binding) {
        blockers.push(JoinBlocker::BuilderBindingMismatch);
    }
    if observation.builder.binding.head_sha != expected.head_sha {
        blockers.push(JoinBlocker::BuilderHeadMismatch);
    }
    if observation.builder.ci_head_sha != expected.head_sha {
        blockers.push(JoinBlocker::BuilderCiHeadMismatch);
    }
    if !observation.builder.ci_green {
        blockers.push(JoinBlocker::BuilderCiNotGreen);
    }
    if observation.ci.head_sha != expected.head_sha {
        blockers.push(JoinBlocker::CiHeadMismatch);
    }
    if !observation.ci.completed {
        blockers.push(JoinBlocker::CiIncomplete);
    }
    if observation.ci.hollow {
        blockers.push(JoinBlocker::CiHollow);
    }
    if observation.ci.rate_limited {
        blockers.push(JoinBlocker::CiRateLimited);
    }
    if !observation.ci.required_checks_green {
        blockers.push(JoinBlocker::CiNotGreen);
    }
    check_review(
        expected,
        &observation.general_review,
        ReviewRole::General,
        &mut blockers,
    );
    check_review(
        expected,
        &observation.secperf_review,
        ReviewRole::SecurityPerformance,
        &mut blockers,
    );
    for finding in observation.findings.iter().filter(|item| item.mandatory) {
        match finding.resolution_head_sha {
            None => blockers.push(JoinBlocker::FindingResolutionMissing),
            Some(head) if head != expected.head_sha => {
                blockers.push(JoinBlocker::FindingResolutionHeadMismatch);
            }
            Some(_) => {}
        }
        if !finding.origin_confirmed {
            blockers.push(JoinBlocker::FindingNotOriginConfirmed);
        }
    }
    if observation.open_blocking_threads > 0 {
        blockers.push(JoinBlocker::BlockingThreadsOpen);
    }
    if !observation.pip_owned {
        blockers.push(JoinBlocker::ForeignOwnership);
    }
    if !observation.mergeable {
        blockers.push(JoinBlocker::NotMergeable);
    }
    if !observation.authorization_valid {
        blockers.push(JoinBlocker::AuthorizationInvalid);
    }
    ExactHeadDecision {
        ready: blockers.is_empty(),
        blockers,
    }
}
