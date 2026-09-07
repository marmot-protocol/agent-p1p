//! Controller-owned publication of independent exact-head review results.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{ReviewOutcome, ReviewResult, WorkerResult, WorkerRole};
use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_github::{
    GitHubError, GitHubWriter, MutationResult, MutationTransport, ReviewEvent, ReviewMutationSpec,
};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;

use crate::RepositoryPolicy;

const PUBLISH_EFFECT: &str = "PUBLISH_REVIEWS";

pub trait ReviewWriter {
    fn ensure_review(&self, spec: &ReviewMutationSpec) -> Result<MutationResult, GitHubError>;
}

impl<T: MutationTransport> ReviewWriter for GitHubWriter<T> {
    fn ensure_review(&self, spec: &ReviewMutationSpec) -> Result<MutationResult, GitHubError> {
        self.ensure_pull_request_review(spec)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ReviewPublicationCycle {
    Idle,
    AuthorizationBlocked,
    Published {
        case_key: String,
        general_review_id: u64,
        secperf_review_id: u64,
    },
}

#[derive(Debug)]
pub enum ReviewPublicationError {
    GitHub(GitHubError),
    Store(StoreError),
    Controller(ControllerError),
    MissingReviewActor,
    InvalidCase,
    InvalidReviewJoin,
    UnexpectedMutationResult,
    Serialization(String),
}

impl fmt::Display for ReviewPublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::MissingReviewActor => {
                formatter.write_str("review publication requires two role actor identities")
            }
            Self::InvalidCase => formatter.write_str("review publication case is invalid"),
            Self::InvalidReviewJoin => {
                formatter.write_str("review publication requires one exact result per role")
            }
            Self::UnexpectedMutationResult => {
                formatter.write_str("review mutation returned a non-review result")
            }
            Self::Serialization(error) => {
                write!(
                    formatter,
                    "review publication serialization failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for ReviewPublicationError {}

macro_rules! error_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for ReviewPublicationError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(GitHubError, GitHub);
error_from!(StoreError, Store);
error_from!(ControllerError, Controller);

#[allow(clippy::too_many_arguments)]
pub fn publish_reviews_once<G: ReviewWriter, S: ReviewWriter>(
    general_writer: &G,
    secperf_writer: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<ReviewPublicationCycle, ReviewPublicationError> {
    if !authorization_valid {
        return Ok(ReviewPublicationCycle::AuthorizationBlocked);
    }
    let general_actor = policy
        .github
        .reviewer_general_actor_id
        .ok_or(ReviewPublicationError::MissingReviewActor)?;
    let secperf_actor = policy
        .github
        .reviewer_secperf_actor_id
        .ok_or(ReviewPublicationError::MissingReviewActor)?;
    let Some(claimed) = store.claim_repository_effect_matching(
        policy.repository.id,
        owner,
        now,
        lease_seconds,
        &[PUBLISH_EFFECT],
    )?
    else {
        return Ok(ReviewPublicationCycle::Idle);
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or(ReviewPublicationError::InvalidCase)?;
    if case.repository_id != policy.repository.id
        || case.policy_revision != policy.revision
        || case.state_revision != claimed.state_revision
        || !matches!(
            case.state.as_str(),
            "FINAL_REVIEW" | "REMEDIATING" | "ESCALATED"
        )
        || case.pr_number.is_none()
        || case.head_sha.is_none()
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(ReviewPublicationError::InvalidCase);
    }
    let (general_reviews, secperf_reviews) = match joined_reviews(store, &case, policy) {
        Ok(reviews) => reviews,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error);
        }
    };
    let general = publish_one(
        general_writer,
        policy,
        &case,
        &claimed.effect_id,
        general_actor,
        "reviewer-general",
        &general_reviews,
    );
    // Both lanes already have accepted exact-head results. Publish each
    // independently; a temporary App outage must not hide the healthy lane.
    // Their stable markers make a partial external success safe to retry.
    let secperf = publish_one(
        secperf_writer,
        policy,
        &case,
        &claimed.effect_id,
        secperf_actor,
        "reviewer-secperf",
        &secperf_reviews,
    );
    let (general_review_id, secperf_review_id) =
        match general.and_then(|id| secperf.map(|other| (id, other))) {
            Ok(ids) => ids,
            Err(error) => {
                store.release_effect(&claimed.effect_id, owner)?;
                return Err(error);
            }
        };
    let evidence = EvidenceInput {
        evidence_id: format!("evidence-reviews-{}", claimed.effect_id),
        kind: "GITHUB_REVIEW_PUBLICATION".into(),
        source: format!("github-pr-{}", case.pr_number.expect("validated PR")),
        payload: json!({
            "head_sha": case.head_sha,
            "general": {
                "actor_id": general_actor,
                "review_id": general_review_id,
                "reviewer_ids": general_reviews
                    .iter()
                    .map(|review| review.reviewer_id.as_str())
                    .collect::<Vec<_>>(),
            },
            "secperf": {
                "actor_id": secperf_actor,
                "review_id": secperf_review_id,
                "reviewer_ids": secperf_reviews
                    .iter()
                    .map(|review| review.reviewer_id.as_str())
                    .collect::<Vec<_>>(),
            },
        }),
    };
    if case.state == "ESCALATED" {
        store.complete_effect_evidence(&claimed.effect_id, owner, now, &evidence)?;
    } else {
        let command = published_command(&case, now, evidence)?;
        LedgerController::apply(store, &policy.case_policy(), &command)?;
    }
    Ok(ReviewPublicationCycle::Published {
        case_key: case.case_key,
        general_review_id,
        secperf_review_id,
    })
}

fn joined_reviews(
    store: &Store,
    case: &StoredCase,
    policy: &RepositoryPolicy,
) -> Result<(Vec<ReviewResult>, Vec<ReviewResult>), ReviewPublicationError> {
    let pr_number = case.pr_number.ok_or(ReviewPublicationError::InvalidCase)?;
    let head_sha = case
        .head_sha
        .as_deref()
        .ok_or(ReviewPublicationError::InvalidCase)?;
    let mut candidates = Vec::new();
    for stored in store.runs_for_case(&case.case_key)? {
        let result: WorkerResult = serde_json::from_value(stored.payload)
            .map_err(|error| ReviewPublicationError::Serialization(error.to_string()))?;
        if let WorkerResult::Review(review) = result
            && review.plan_version == case.plan_version
            && review.pr_number == pr_number
            && review.reviewed_head_sha == head_sha
        {
            candidates.push(review);
        }
    }
    let round = candidates
        .iter()
        .map(|review| review.review_round)
        .max()
        .ok_or(ReviewPublicationError::InvalidReviewJoin)?;
    let workflow = policy
        .workflow_policy()
        .map_err(|error| ReviewPublicationError::Serialization(error.to_string()))?;
    let required = workflow
        .required_reviewers()
        .map(|role| {
            (
                role.reviewer_id
                    .as_deref()
                    .ok_or(ReviewPublicationError::InvalidReviewJoin),
                role.role,
            )
        })
        .map(|(id, role)| id.map(|id| (id, role)))
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let mut selected = std::collections::BTreeMap::new();
    for review in candidates
        .into_iter()
        .filter(|review| review.review_round == round)
    {
        if !matches!(
            review.outcome,
            ReviewOutcome::Approve | ReviewOutcome::RequestChanges
        ) {
            return Err(ReviewPublicationError::InvalidReviewJoin);
        }
        if required.get(review.reviewer_id.as_str()) != Some(&review.common.role)
            || selected
                .insert(review.reviewer_id.clone(), review)
                .is_some()
        {
            return Err(ReviewPublicationError::InvalidReviewJoin);
        }
    }
    if selected.len() != required.len() {
        return Err(ReviewPublicationError::InvalidReviewJoin);
    }
    let mut general = Vec::new();
    let mut secperf = Vec::new();
    for (reviewer_id, role) in required {
        let review = selected
            .remove(reviewer_id)
            .ok_or(ReviewPublicationError::InvalidReviewJoin)?;
        match role {
            WorkerRole::ReviewerGeneral => general.push(review),
            WorkerRole::ReviewerSecperf => secperf.push(review),
            _ => return Err(ReviewPublicationError::InvalidReviewJoin),
        }
    }
    if general.is_empty() || secperf.is_empty() {
        return Err(ReviewPublicationError::InvalidReviewJoin);
    }
    Ok((general, secperf))
}

#[allow(clippy::too_many_arguments)]
fn publish_one<W: ReviewWriter>(
    writer: &W,
    policy: &RepositoryPolicy,
    case: &StoredCase,
    effect_id: &str,
    actor_id: u64,
    role_name: &str,
    reviews: &[ReviewResult],
) -> Result<u64, ReviewPublicationError> {
    let head = reviews
        .first()
        .ok_or(ReviewPublicationError::InvalidReviewJoin)?
        .reviewed_head_sha
        .clone();
    if reviews
        .iter()
        .any(|review| review.reviewed_head_sha != head)
    {
        return Err(ReviewPublicationError::InvalidReviewJoin);
    }
    let event = if reviews
        .iter()
        .any(|review| review.outcome == ReviewOutcome::RequestChanges)
    {
        ReviewEvent::RequestChanges
    } else if reviews
        .iter()
        .all(|review| review.outcome == ReviewOutcome::Approve)
    {
        ReviewEvent::Approve
    } else {
        return Err(ReviewPublicationError::InvalidReviewJoin);
    };
    let reports = reviews
        .iter()
        .map(render_review_summary)
        .collect::<Vec<_>>()
        .join("\n\n");
    let lane = if role_name == "reviewer-general" {
        "General review"
    } else {
        "Security and performance review"
    };
    let body = format!(
        "## {lane}: {}\n\nReviewed commit: `{head}`\n\n{reports}\n\nChecks are reviewer-reported, not independently rerun by Pip. Full structured evidence is retained by Pip.\n\nPip reviewer role: {role_name}",
        match event {
            ReviewEvent::Approve => "Approved",
            ReviewEvent::RequestChanges => "Changes requested",
            _ => return Err(ReviewPublicationError::InvalidReviewJoin),
        },
    );
    let result = writer.ensure_review(&ReviewMutationSpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        pull_request_number: case.pr_number.ok_or(ReviewPublicationError::InvalidCase)?,
        effect_id: format!("{effect_id}:{role_name}"),
        expected_actor_id: actor_id,
        expected_head_sha: head,
        body,
        event,
    })?;
    match result {
        MutationResult::Created(id) | MutationResult::Existing(id) => Ok(id),
        MutationResult::Updated(_) | MutationResult::Merged(_) => {
            Err(ReviewPublicationError::UnexpectedMutationResult)
        }
    }
}

fn render_review_summary(review: &ReviewResult) -> String {
    use crate::publication_text::{bullets, prose, summaries};
    let mut body = format!(
        "### {} — {} ({})\n\n{}",
        prose(&review.reviewer_id),
        prose(&review.common.requested_model),
        if review.outcome == ReviewOutcome::Approve {
            "Approved"
        } else {
            "Changes requested"
        },
        if review.blocking_findings.is_empty() {
            "No blocking findings."
        } else {
            "Blocking findings:"
        }
    );
    for finding in &review.blocking_findings {
        body.push_str(&format!("\n\n**{}** ({})\n\n{}\n\nImpact: {}\n\nRequested change: {}\n\nVerification needed:\n\n{}",
            prose(&finding.summary), prose(&finding.id), prose(&finding.defect),
            prose(&finding.consequence), prose(&finding.corrective_direction),
            bullets(&finding.required_evidence, "No additional verification specified.")));
    }
    if !review.suggestions.is_empty() {
        body.push_str(&format!(
            "\n\n**Suggestions**\n\n{}",
            bullets(
                review
                    .suggestions
                    .iter()
                    .map(|item| format!("{} — {}", item.summary, item.rationale)),
                ""
            )
        ));
    }
    for (label, key, empty) in [
        ("Scope", "review_scope", ""),
        ("Summary", "summary", ""),
        (
            "Reviewer-reported checks",
            "local_checks",
            "No readable check summary supplied; do not infer that tests ran.",
        ),
        (
            "Limitations",
            "limitations",
            "No limitation summary supplied.",
        ),
        ("Earlier feedback", "prior_suggestions", ""),
    ] {
        let lines = review
            .common
            .evidence
            .get(key)
            .map(summaries)
            .unwrap_or_default();
        if !lines.is_empty() || !empty.is_empty() {
            body.push_str(&format!("\n\n**{label}**\n\n{}", bullets(lines, empty)));
        }
    }
    body
}

fn published_command(
    case: &StoredCase,
    now: u64,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, ReviewPublicationError> {
    let case_id = CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(case.repository_id).ok_or(ReviewPublicationError::InvalidCase)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(case.issue_number).ok_or(ReviewPublicationError::InvalidCase)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(ReviewPublicationError::InvalidCase)?,
        ),
    );
    Ok(WorkflowCommand {
        case_id,
        event_id: EventId::from_str(&format!(
            "event-reviews-published-repo{}-issue{}-workflow{}-revision{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| ReviewPublicationError::InvalidCase)?,
        observed_at: ObservedAt::new(now),
        expected_state: CaseState::from_str(&case.state)
            .map_err(|_| ReviewPublicationError::InvalidCase)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(ReviewPublicationError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(ReviewPublicationError::InvalidCase)?,
        ),
        remediation_round: case.remediation_round,
        plan_version: NonZeroU32::new(case.plan_version).map(PlanVersion::new),
        pr_number: case
            .pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        head_sha: case
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| ReviewPublicationError::InvalidCase)?,
        event: Event::ReviewsPublished,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: json!({
            "general_review_id": evidence.payload["general"]["review_id"],
            "secperf_review_id": evidence.payload["secperf"]["review_id"],
            "head_sha": case.head_sha,
        }),
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}
