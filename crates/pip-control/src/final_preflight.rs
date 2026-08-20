//! Durable final-review preflight over exact ledger and GitHub evidence.

use std::collections::BTreeMap;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{
    BuilderOutcome, ConfirmationStatus, PlannerOutcome, ReviewOutcome, WorkerResult, WorkerRole,
};
use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_github::{
    CiVerdict, GitHubError, GitHubReader, PullRequestEvidence, ReadTransport, ReviewSnapshot,
    ReviewState, ReviewThreadSnapshot, evaluate_ci,
};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;

use crate::{PullRequestSource, RepositoryPolicy};

const OBSERVE_EFFECT: &str = "OBSERVE_FINAL_PREFLIGHT";
const REVIEW_ROLES: [(WorkerRole, &str); 2] = [
    (WorkerRole::ReviewerGeneral, "reviewer-general"),
    (WorkerRole::ReviewerSecperf, "reviewer-secperf"),
];

pub trait FinalPreflightSource: PullRequestSource {
    fn review_threads(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<Vec<ReviewThreadSnapshot>, GitHubError>;
}

impl<T: ReadTransport> FinalPreflightSource for GitHubReader<T> {
    fn review_threads(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<Vec<ReviewThreadSnapshot>, GitHubError> {
        self.read_review_threads(owner, repository, repository_id, pull_request_number)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FinalPreflightCycle {
    Idle,
    AuthorizationBlocked,
    Pending {
        case_key: String,
        blockers: Vec<String>,
    },
    Accepted {
        case_key: String,
        head_sha: String,
    },
}

#[derive(Debug)]
pub enum FinalPreflightError {
    GitHub(GitHubError),
    Store(StoreError),
    Controller(ControllerError),
    MissingAutomationActor,
    InvalidCase,
    InvalidWorkerEvidence(String),
}

impl fmt::Display for FinalPreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::MissingAutomationActor => {
                formatter.write_str("final preflight requires a GitHub automation actor")
            }
            Self::InvalidCase => formatter.write_str("final preflight case binding is invalid"),
            Self::InvalidWorkerEvidence(error) => {
                write!(
                    formatter,
                    "final preflight worker evidence is invalid: {error}"
                )
            }
        }
    }
}

impl std::error::Error for FinalPreflightError {}

macro_rules! error_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for FinalPreflightError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(GitHubError, GitHub);
error_from!(StoreError, Store);
error_from!(ControllerError, Controller);

pub fn reconcile_final_preflight_once<S: FinalPreflightSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    observed_at: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<FinalPreflightCycle, FinalPreflightError> {
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(FinalPreflightError::MissingAutomationActor)?;
    if !authorization_valid {
        return Ok(FinalPreflightCycle::AuthorizationBlocked);
    }
    let Some(claimed) =
        store.claim_effect_matching(owner, observed_at, lease_seconds, &[OBSERVE_EFFECT])?
    else {
        return Ok(FinalPreflightCycle::Idle);
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or(FinalPreflightError::InvalidCase)?;
    if case.state != "FINAL_REVIEW"
        || case.state_revision != claimed.state_revision
        || case.repository_id != policy.repository.id
        || case.policy_revision != policy.revision
        || case.pr_number.is_none()
        || case.head_sha.is_none()
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(FinalPreflightError::InvalidCase);
    }
    let pr_number = case.pr_number.ok_or(FinalPreflightError::InvalidCase)?;
    let head_sha = case
        .head_sha
        .as_deref()
        .ok_or(FinalPreflightError::InvalidCase)?;
    let evidence = match source.pull_request(
        &policy.repository.owner,
        &policy.repository.name,
        policy.repository.id,
        pr_number,
    ) {
        Ok(evidence) => evidence,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    let threads = match source.review_threads(
        &policy.repository.owner,
        &policy.repository.name,
        policy.repository.id,
        pr_number,
    ) {
        Ok(threads) => threads,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    let mut blockers = Vec::new();
    if let Err(error) =
        validate_pull_request(&case, policy, expected_actor, &evidence, &mut blockers)
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(error);
    }
    if let Err(error) = validate_ledger_join(store, &case, &mut blockers) {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(error);
    }
    validate_published_reviews(expected_actor, head_sha, &evidence, &mut blockers);
    for thread in &threads {
        if !thread.is_resolved {
            push_unique(
                &mut blockers,
                format!("UNRESOLVED_REVIEW_THREAD:{}", thread.id),
            );
        }
    }
    if !blockers.is_empty() {
        store.release_effect(&claimed.effect_id, owner)?;
        return Ok(FinalPreflightCycle::Pending {
            case_key: case.case_key,
            blockers,
        });
    }
    let github_payload = json!({
        "pull_request": evidence,
        "review_threads": threads,
        "automation_actor_id": expected_actor,
        "required_role_stamps": REVIEW_ROLES.map(|(_, role)| role),
    });
    let command = workflow(
        &case,
        policy,
        observed_at,
        EvidenceInput {
            evidence_id: format!(
                "evidence-final-preflight-repo{}-issue{}-workflow{}-revision{}",
                case.repository_id, case.issue_number, case.workflow_version, case.state_revision
            ),
            kind: "GITHUB_FINAL_PREFLIGHT".into(),
            source: format!("github-pr-{pr_number}"),
            payload: github_payload,
        },
    )?;
    LedgerController::apply(store, &policy.case_policy(), &command)?;
    Ok(FinalPreflightCycle::Accepted {
        case_key: case.case_key,
        head_sha: head_sha.into(),
    })
}

fn validate_pull_request(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    expected_actor: u64,
    evidence: &PullRequestEvidence,
    blockers: &mut Vec<String>,
) -> Result<(), FinalPreflightError> {
    let pull = &evidence.pull_request;
    let pr_number = case.pr_number.ok_or(FinalPreflightError::InvalidCase)?;
    let head_sha = case
        .head_sha
        .as_deref()
        .ok_or(FinalPreflightError::InvalidCase)?;
    let expected_branch = format!(
        "{}repo-{}/issue-{}/workflow-{}",
        policy.branch_prefix, case.repository_id, case.issue_number, case.workflow_version
    );
    if pull.number != pr_number
        || pull.head_repository_id != policy.repository.id
        || pull.head_repository != policy.repository.full_name()
        || pull.base_branch != policy.repository.default_branch
        || pull.head_branch != expected_branch
        || pull.head_sha != head_sha
        || pull.author_id != expected_actor
    {
        push_unique(blockers, "PULL_REQUEST_IDENTITY_DRIFT".into());
    }
    if !pull.open || !pull.draft || pull.merged {
        push_unique(blockers, "PULL_REQUEST_DISPOSITION_DRIFT".into());
    }
    if pull.mergeable != Some(true) || pull.mergeable_state != "clean" {
        push_unique(blockers, "PR_NOT_CLEANLY_MERGEABLE".into());
    }
    let evaluation = evaluate_ci(evidence, head_sha, &policy.required_ci_contexts);
    if evaluation.verdict != CiVerdict::Accepted {
        if evaluation.blockers.is_empty() {
            push_unique(blockers, "CI_NOT_ACCEPTED".into());
        }
        for blocker in evaluation.blockers {
            push_unique(blockers, format!("CI:{blocker}"));
        }
    }
    Ok(())
}

fn validate_ledger_join(
    store: &Store,
    case: &StoredCase,
    blockers: &mut Vec<String>,
) -> Result<(), FinalPreflightError> {
    let pr_number = case.pr_number.ok_or(FinalPreflightError::InvalidCase)?;
    let head_sha = case
        .head_sha
        .as_deref()
        .ok_or(FinalPreflightError::InvalidCase)?;
    let mut planner = false;
    let mut builder = false;
    let mut resolutions = Vec::new();
    let mut reviews = Vec::new();
    let mut mandatory_findings: BTreeMap<String, WorkerRole> = BTreeMap::new();
    for stored in store.runs_for_case(&case.case_key)? {
        let result: WorkerResult = serde_json::from_value(stored.payload)
            .map_err(|error| FinalPreflightError::InvalidWorkerEvidence(error.to_string()))?;
        match result {
            WorkerResult::Planner(result)
                if result.outcome == PlannerOutcome::Proceed
                    && result.plan_version == case.plan_version =>
            {
                planner = true;
            }
            WorkerResult::Builder(result)
                if result.outcome == BuilderOutcome::ReviewReady
                    && result.plan_version == case.plan_version
                    && result.pr_number == Some(pr_number)
                    && result.head_sha.as_deref() == Some(head_sha) =>
            {
                builder = true;
                resolutions.extend(result.finding_resolutions);
            }
            WorkerResult::Review(result) => {
                for finding in &result.blocking_findings {
                    if mandatory_findings
                        .insert(finding.id.clone(), result.common.role)
                        .is_some_and(|origin| origin != result.common.role)
                    {
                        return Err(FinalPreflightError::InvalidWorkerEvidence(format!(
                            "finding {} has conflicting origin roles",
                            finding.id
                        )));
                    }
                }
                if result.plan_version == case.plan_version
                    && result.pr_number == pr_number
                    && result.reviewed_head_sha == head_sha
                {
                    reviews.push(result);
                }
            }
            _ => {}
        }
    }
    if !planner {
        push_unique(blockers, "MISSING_ACCEPTED_PLAN_RESULT".into());
    }
    if !builder {
        push_unique(blockers, "MISSING_EXACT_BUILDER_RESULT".into());
    }
    let round = reviews.iter().map(|review| review.review_round).max();
    for (role, name) in REVIEW_ROLES {
        let matching = reviews
            .iter()
            .filter(|review| Some(review.review_round) == round && review.common.role == role)
            .collect::<Vec<_>>();
        if matching.len() != 1
            || matching[0].outcome != ReviewOutcome::Approve
            || !matching[0].blocking_findings.is_empty()
        {
            push_unique(blockers, format!("MISSING_LEDGER_APPROVAL:{name}"));
        }
    }
    for (finding_id, origin_role) in mandatory_findings {
        if !resolutions.iter().any(|resolution| {
            resolution.finding_id == finding_id && resolution.resolved_head_sha == head_sha
        }) {
            push_unique(blockers, format!("MISSING_FINDING_RESOLUTION:{finding_id}"));
        }
        if !reviews.iter().any(|review| {
            review.common.role == origin_role
                && review.finding_confirmations.iter().any(|confirmation| {
                    confirmation.finding_id == finding_id
                        && confirmation.status == ConfirmationStatus::ConfirmedResolved
                        && confirmation.reviewed_fix_sha == head_sha
                })
        }) {
            push_unique(
                blockers,
                format!("MISSING_ORIGIN_CONFIRMATION:{finding_id}"),
            );
        }
    }
    Ok(())
}

fn validate_published_reviews(
    expected_actor: u64,
    head_sha: &str,
    evidence: &PullRequestEvidence,
    blockers: &mut Vec<String>,
) {
    let mut latest: BTreeMap<&str, &ReviewSnapshot> = BTreeMap::new();
    for review in &evidence.reviews {
        if review.actor_id != expected_actor
            || !review.exact_head
            || review.commit_id.as_deref() != Some(head_sha)
            || review.submitted_at.as_deref().is_none_or(str::is_empty)
        {
            continue;
        }
        let Some(role) = stamped_role(&review.body) else {
            continue;
        };
        let replace = latest.get(role).is_none_or(|prior| {
            (review.submitted_at.as_deref(), review.id) > (prior.submitted_at.as_deref(), prior.id)
        });
        if replace {
            latest.insert(role, review);
        }
    }
    for (_, role) in REVIEW_ROLES {
        if latest.get(role).map(|review| review.state) != Some(ReviewState::Approved) {
            push_unique(blockers, format!("MISSING_EXACT_HEAD_APPROVAL:{role}"));
        }
    }
}

fn stamped_role(body: &str) -> Option<&str> {
    let mut roles = body.lines().filter_map(|line| {
        REVIEW_ROLES
            .iter()
            .map(|(_, role)| *role)
            .find(|role| line == format!("Pip reviewer role: {role}"))
    });
    let role = roles.next()?;
    roles.next().is_none().then_some(role)
}

fn workflow(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    observed_at: u64,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, FinalPreflightError> {
    let case_id = CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(case.repository_id).ok_or(FinalPreflightError::InvalidCase)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(case.issue_number).ok_or(FinalPreflightError::InvalidCase)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(FinalPreflightError::InvalidCase)?,
        ),
    );
    if case_id.to_string() != case.case_key || case.policy_revision != policy.revision {
        return Err(FinalPreflightError::InvalidCase);
    }
    Ok(WorkflowCommand {
        case_id,
        event_id: EventId::from_str(&format!(
            "event-final-preflight-repo{}-issue{}-workflow{}-revision{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| FinalPreflightError::InvalidCase)?,
        observed_at: ObservedAt::new(observed_at),
        expected_state: CaseState::FinalReview,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(FinalPreflightError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(FinalPreflightError::InvalidCase)?,
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
            .map_err(|_| FinalPreflightError::InvalidCase)?,
        event: Event::FinalPreflightAccepted,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: json!({
            "verdict": "ACCEPTED",
            "pull_request_number": case.pr_number,
            "head_sha": case.head_sha,
        }),
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}
