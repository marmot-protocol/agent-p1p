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

use crate::{IntakeSource, PullRequestSource, RepositoryPolicy};

const OBSERVE_EFFECT: &str = "OBSERVE_FINAL_PREFLIGHT";
const REVIEW_ROLES: [(WorkerRole, &str); 2] = [
    (WorkerRole::ReviewerGeneral, "reviewer-general"),
    (WorkerRole::ReviewerSecperf, "reviewer-secperf"),
];

pub trait FinalPreflightSource: PullRequestSource + IntakeSource {
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

pub fn reconcile_final_preflight_once<'a, S: FinalPreflightSource>(
    source: &S,
    scope: impl Into<crate::RepositoryScope<'a>>,
    store: &mut Store,
    observed_at: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<FinalPreflightCycle, FinalPreflightError> {
    let scope = scope.into();
    let policy = scope.policy;
    if !authorization_valid {
        return Ok(FinalPreflightCycle::AuthorizationBlocked);
    }
    let Some(claimed) = scope.claim(store, owner, observed_at, lease_seconds, &[OBSERVE_EFFECT])?
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
    let issue_authorization = match source.intake(
        &policy.repository.owner,
        &policy.repository.name,
        case.issue_number,
    ) {
        Ok(evidence) => evidence,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    if !fresh_issue_authorization(policy, &case, &issue_authorization) {
        store.release_effect(&claimed.effect_id, owner)?;
        return Ok(FinalPreflightCycle::AuthorizationBlocked);
    }
    let blockers = match final_gate_blockers(store, &case, policy, &evidence, &threads, Some(true))
    {
        Ok(blockers) => blockers,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error);
        }
    };
    if !blockers.is_empty() {
        store.release_effect(&claimed.effect_id, owner)?;
        return Ok(FinalPreflightCycle::Pending {
            case_key: case.case_key,
            blockers,
        });
    }
    let github_payload = json!({
        "issue_authorization": issue_authorization,
        "pull_request": evidence,
        "review_threads": threads,
        "automation_actor_id": policy.github.automation_actor_id,
        "review_actor_ids": BTreeMap::from([
            ("reviewer-general", policy.github.reviewer_general_actor_id),
            ("reviewer-secperf", policy.github.reviewer_secperf_actor_id),
        ]),
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

fn fresh_issue_authorization(
    policy: &RepositoryPolicy,
    case: &StoredCase,
    evidence: &pip_github::IntakeSnapshot,
) -> bool {
    let expected_repository = policy.repository.full_name();
    let latest_label = evidence
        .label_events
        .iter()
        .filter(|event| event.label == policy.intake.label)
        .max_by_key(|event| (&event.created_at, event.id));
    evidence.repository.id == policy.repository.id
        && evidence.repository.full_name == expected_repository
        && evidence.repository.default_branch == policy.repository.default_branch
        && evidence.issue.number == case.issue_number
        && evidence.issue.open
        && !evidence.issue.is_pull_request
        && evidence.issue.labels.contains(&policy.intake.label)
        && latest_label.is_some_and(|event| {
            event.labeled && policy.intake.trusted_actor_ids.contains(&event.actor_id)
        })
}

fn validate_pull_request(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    expected_actor: u64,
    evidence: &PullRequestEvidence,
    blockers: &mut Vec<String>,
    expected_draft: Option<bool>,
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
    if !pull.open || pull.merged || expected_draft.is_some_and(|draft| pull.draft != draft) {
        push_unique(blockers, "PULL_REQUEST_DISPOSITION_DRIFT".into());
    }
    match pull.mergeable {
        Some(false) => push_unique(blockers, "PR_MERGE_CONFLICTS".into()),
        None => push_unique(blockers, "PR_MERGEABILITY_UNKNOWN".into()),
        Some(true) => {}
    }
    if pull.mergeable_state != "clean" {
        // A conflict-free head can still be blocked by branch requirements.
        // Preserve GitHub's observation without guessing which rule failed.
        push_unique(blockers, format!("PR_MERGE_STATE:{}", pull.mergeable_state));
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

pub(crate) fn final_gate_blockers(
    store: &Store,
    case: &StoredCase,
    policy: &RepositoryPolicy,
    evidence: &PullRequestEvidence,
    threads: &[ReviewThreadSnapshot],
    expected_draft: Option<bool>,
) -> Result<Vec<String>, FinalPreflightError> {
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(FinalPreflightError::MissingAutomationActor)?;
    let review_actors = [
        (
            "reviewer-general",
            policy
                .github
                .reviewer_general_actor_id
                .ok_or(FinalPreflightError::MissingAutomationActor)?,
        ),
        (
            "reviewer-secperf",
            policy
                .github
                .reviewer_secperf_actor_id
                .ok_or(FinalPreflightError::MissingAutomationActor)?,
        ),
    ];
    let mut blockers = Vec::new();
    validate_pull_request(
        case,
        policy,
        expected_actor,
        evidence,
        &mut blockers,
        expected_draft,
    )?;
    validate_ledger_join(store, case, policy, &mut blockers)?;
    let head = case
        .head_sha
        .as_deref()
        .ok_or(FinalPreflightError::InvalidCase)?;
    validate_published_reviews(&review_actors, head, evidence, &mut blockers);
    for thread in threads {
        if !thread.is_resolved {
            push_unique(
                &mut blockers,
                format!("UNRESOLVED_REVIEW_THREAD:{}", thread.id),
            );
        }
    }
    Ok(blockers)
}

fn validate_ledger_join(
    store: &Store,
    case: &StoredCase,
    policy: &RepositoryPolicy,
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
    let workflow = policy
        .workflow_policy()
        .map_err(|error| FinalPreflightError::InvalidWorkerEvidence(error.to_string()))?;
    let required_reviewers = workflow
        .required_reviewers()
        .map(|role| {
            role.reviewer_id
                .as_ref()
                .map(|id| (id.clone(), role.role))
                .ok_or_else(|| {
                    FinalPreflightError::InvalidWorkerEvidence(
                        "required reviewer is missing an instance id".into(),
                    )
                })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut mandatory_findings: BTreeMap<String, String> = BTreeMap::new();
    let history = store.immutable_history_for_case(&case.case_key)?;
    let published_build = crate::draft_pr::published_builder(&history, case)
        .map_err(|error| FinalPreflightError::InvalidWorkerEvidence(error.to_string()))?;
    let source_head = published_build
        .as_ref()
        .and_then(|build| build.head_sha.as_deref())
        .unwrap_or(head_sha);
    for stored in history.runs {
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
                    && result.head_sha.as_deref() == Some(source_head)
                    && published_build
                        .as_ref()
                        .is_none_or(|build| *build == result) =>
            {
                builder = true;
                resolutions.extend(result.finding_resolutions);
            }
            WorkerResult::Review(result) => {
                for finding in &result.blocking_findings {
                    if mandatory_findings
                        .insert(finding.id.clone(), result.reviewer_id.clone())
                        .is_some_and(|origin| origin != result.reviewer_id)
                    {
                        return Err(FinalPreflightError::InvalidWorkerEvidence(format!(
                            "finding {} has conflicting reviewer origins",
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
    let round = reviews
        .iter()
        .filter(|review| required_reviewers.contains_key(&review.reviewer_id))
        .map(|review| review.review_round)
        .max();
    for (reviewer_id, role) in &required_reviewers {
        let matching = reviews
            .iter()
            .filter(|review| {
                Some(review.review_round) == round
                    && review.common.role == *role
                    && review.reviewer_id == *reviewer_id
            })
            .collect::<Vec<_>>();
        if matching.len() != 1
            || matching[0].outcome != ReviewOutcome::Approve
            || !matching[0].blocking_findings.is_empty()
        {
            push_unique(blockers, format!("MISSING_LEDGER_APPROVAL:{reviewer_id}"));
        }
    }
    for (finding_id, origin_reviewer) in mandatory_findings {
        if !resolutions.iter().any(|resolution| {
            resolution.finding_id == finding_id && resolution.resolved_head_sha == source_head
        }) {
            push_unique(blockers, format!("MISSING_FINDING_RESOLUTION:{finding_id}"));
        }
        if !reviews.iter().any(|review| {
            review.reviewer_id == origin_reviewer
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
    review_actors: &[(&str, u64); 2],
    head_sha: &str,
    evidence: &PullRequestEvidence,
    blockers: &mut Vec<String>,
) {
    let mut latest: BTreeMap<&str, &ReviewSnapshot> = BTreeMap::new();
    for review in &evidence.reviews {
        if !review.exact_head
            || review.commit_id.as_deref() != Some(head_sha)
            || review.submitted_at.as_deref().is_none_or(str::is_empty)
        {
            continue;
        }
        let Some(role) = stamped_role(&review.body) else {
            continue;
        };
        if review_actors
            .iter()
            .find(|(expected_role, _)| *expected_role == role)
            .map(|(_, actor_id)| *actor_id)
            != Some(review.actor_id)
        {
            continue;
        }
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
