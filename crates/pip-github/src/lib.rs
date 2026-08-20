//! Read-only GitHub evidence adapter.

#![forbid(unsafe_code)]

mod write;

pub use write::{
    CommentSpec, GitHubWriter, MergeModePolicy, MergeSpec, MutationRequest, MutationResult,
    MutationTransport, PullRequestSpec, ReviewEvent, ReviewMutationSpec,
};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub max_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub trait ReadTransport {
    fn get(&self, request: ReadRequest) -> Result<ReadResponse, GitHubError>;
}

#[derive(Clone)]
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { agent }
    }
}

impl ReadTransport for UreqTransport {
    fn get(&self, request: ReadRequest) -> Result<ReadResponse, GitHubError> {
        if request.method != "GET" {
            return Err(GitHubError::Transport(
                "read transport received a non-GET request".into(),
            ));
        }
        let mut call = self.agent.get(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let mut response = call
            .call()
            .map_err(|error| GitHubError::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
            })
            .collect();
        let body_limit = u64::try_from(request.max_bytes.saturating_add(1)).unwrap_or(u64::MAX);
        let body = response
            .body_mut()
            .with_config()
            .limit(body_limit)
            .read_to_vec()
            .map_err(|error| GitHubError::Transport(error.to_string()))?;
        Ok(ReadResponse {
            status,
            headers,
            body,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitHubError {
    InvalidConfiguration,
    InvalidRepository,
    InvalidIssueNumber,
    HttpStatus(u16),
    ResponseTooLarge,
    MalformedJson(String),
    InvalidIdentity,
    PaginationLimit,
    UnsafePaginationUrl,
    InvalidMutation,
    IdempotencyConflict,
    DuplicateOwnership,
    OwnershipConflict,
    MutationDisabled,
    Transport(String),
}

impl fmt::Display for GitHubError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid GitHub reader configuration")
            }
            Self::InvalidRepository => formatter.write_str("invalid repository owner or name"),
            Self::InvalidIssueNumber => formatter.write_str("issue number must be positive"),
            Self::HttpStatus(status) => write!(formatter, "GitHub returned HTTP {status}"),
            Self::ResponseTooLarge => {
                formatter.write_str("GitHub response exceeded the configured bound")
            }
            Self::MalformedJson(error) => write!(formatter, "malformed GitHub JSON: {error}"),
            Self::InvalidIdentity => {
                formatter.write_str("GitHub response lacks canonical numeric identity")
            }
            Self::PaginationLimit => formatter.write_str("GitHub pagination limit exceeded"),
            Self::UnsafePaginationUrl => {
                formatter.write_str("GitHub pagination attempted to change origin")
            }
            Self::InvalidMutation => formatter.write_str("invalid GitHub mutation request"),
            Self::IdempotencyConflict => {
                formatter.write_str("GitHub idempotency marker conflicts with existing content")
            }
            Self::DuplicateOwnership => {
                formatter.write_str("multiple GitHub objects claim the same Pip ownership")
            }
            Self::OwnershipConflict => {
                formatter.write_str("GitHub object has a Pip marker from an unexpected actor")
            }
            Self::MutationDisabled => formatter.write_str("GitHub mutation is disabled by policy"),
            Self::Transport(error) => write!(formatter, "GitHub transport failed: {error}"),
        }
    }
}

impl std::error::Error for GitHubError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RepositorySnapshot {
    pub id: u64,
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IssueSnapshot {
    pub id: u64,
    pub number: u64,
    pub open: bool,
    pub is_pull_request: bool,
    pub labels: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LabelEvent {
    pub id: u64,
    pub labeled: bool,
    pub actor_id: u64,
    pub label: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntakeSnapshot {
    pub repository: RepositorySnapshot,
    pub issue: IssueSnapshot,
    pub label_events: Vec<LabelEvent>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
    Waiting,
    Requested,
    Pending,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckConclusion {
    ActionRequired,
    Cancelled,
    Failure,
    Neutral,
    Skipped,
    Stale,
    StartupFailure,
    Success,
    TimedOut,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CheckRunSnapshot {
    pub id: u64,
    pub app_id: u64,
    pub name: String,
    pub head_sha: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitStatusState {
    Error,
    Failure,
    Pending,
    Success,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CommitStatusSnapshot {
    pub id: u64,
    pub creator_id: u64,
    pub context: String,
    pub state: CommitStatusState,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewSnapshot {
    pub id: u64,
    pub actor_id: u64,
    pub state: ReviewState,
    pub commit_id: Option<String>,
    pub exact_head: bool,
    pub submitted_at: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PullRequestSnapshot {
    pub id: u64,
    pub number: u64,
    pub open: bool,
    pub draft: bool,
    pub merged: bool,
    pub mergeable: Option<bool>,
    pub mergeable_state: String,
    pub author_id: u64,
    pub head_repository_id: u64,
    pub head_repository: String,
    pub head_branch: String,
    pub head_sha: String,
    pub base_branch: String,
    pub base_sha: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PullRequestEvidence {
    pub pull_request: PullRequestSnapshot,
    pub check_runs: Vec<CheckRunSnapshot>,
    pub commit_status_state: CommitStatusState,
    pub commit_statuses: Vec<CommitStatusSnapshot>,
    pub reviews: Vec<ReviewSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CiVerdict {
    Accepted,
    Pending,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CiEvaluation {
    pub verdict: CiVerdict,
    pub blockers: Vec<String>,
}

#[must_use]
pub fn evaluate_ci(
    evidence: &PullRequestEvidence,
    expected_head_sha: &str,
    required_contexts: &[String],
) -> CiEvaluation {
    let mut failed = Vec::new();
    let mut pending = Vec::new();
    if evidence.pull_request.head_sha != expected_head_sha {
        push_unique(&mut failed, "PR_HEAD_MISMATCH".into());
    }
    if !evidence.pull_request.open || evidence.pull_request.merged {
        push_unique(&mut failed, "PR_NOT_OPEN".into());
    }
    let historical_failure = evidence.check_runs.iter().any(|check| {
        matches!(
            check.conclusion,
            Some(
                CheckConclusion::ActionRequired
                    | CheckConclusion::Cancelled
                    | CheckConclusion::Failure
                    | CheckConclusion::StartupFailure
                    | CheckConclusion::Stale
                    | CheckConclusion::TimedOut
            )
        )
    }) || evidence.commit_statuses.iter().any(|status| {
        matches!(
            status.state,
            CommitStatusState::Error | CommitStatusState::Failure
        )
    });
    if historical_failure {
        push_unique(&mut failed, "HISTORICAL_FAILED_ATTEMPT".into());
    }
    match evidence.commit_status_state {
        CommitStatusState::Error | CommitStatusState::Failure => {
            push_unique(&mut failed, "COMBINED_STATUS_FAILURE".into());
        }
        CommitStatusState::Pending if !evidence.commit_statuses.is_empty() => {
            push_unique(&mut pending, "CI_PENDING".into());
        }
        CommitStatusState::Pending | CommitStatusState::Success => {}
    }

    if required_contexts.is_empty()
        && evidence.check_runs.is_empty()
        && evidence.commit_statuses.is_empty()
    {
        push_unique(&mut pending, "CI_HOLLOW".into());
    }
    for required in required_contexts {
        let checks = evidence
            .check_runs
            .iter()
            .filter(|check| &check.name == required)
            .collect::<Vec<_>>();
        let statuses = evidence
            .commit_statuses
            .iter()
            .filter(|status| &status.context == required)
            .collect::<Vec<_>>();
        if checks.is_empty() && statuses.is_empty() {
            push_unique(&mut pending, format!("MISSING_REQUIRED_CONTEXT:{required}"));
            continue;
        }
        let in_progress = checks
            .iter()
            .any(|check| check.status != CheckStatus::Completed)
            || statuses
                .iter()
                .any(|status| status.state == CommitStatusState::Pending);
        if in_progress {
            push_unique(&mut pending, "CI_PENDING".into());
        }
        let green = checks.iter().any(|check| {
            check.status == CheckStatus::Completed
                && check.conclusion == Some(CheckConclusion::Success)
        }) || statuses
            .iter()
            .any(|status| status.state == CommitStatusState::Success);
        if !green && !in_progress {
            push_unique(
                &mut failed,
                format!("REQUIRED_CONTEXT_NOT_GREEN:{required}"),
            );
        }
    }
    if evidence
        .check_runs
        .iter()
        .any(|check| check.head_sha != expected_head_sha)
    {
        push_unique(&mut failed, "CHECK_HEAD_MISMATCH".into());
    }
    if !failed.is_empty() {
        CiEvaluation {
            verdict: CiVerdict::Failed,
            blockers: failed,
        }
    } else if !pending.is_empty() {
        CiEvaluation {
            verdict: CiVerdict::Pending,
            blockers: pending,
        }
    } else {
        CiEvaluation {
            verdict: CiVerdict::Accepted,
            blockers: Vec::new(),
        }
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueCommentSnapshot {
    pub id: u64,
    pub actor_id: u64,
    pub issue_number: u64,
    pub html_url: String,
    pub body: String,
    pub body_sha256: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
struct RepositoryDto {
    id: u64,
    full_name: String,
    default_branch: String,
}

#[derive(Deserialize)]
struct LabelDto {
    name: String,
}

#[derive(Deserialize)]
struct IssueDto {
    id: u64,
    number: u64,
    state: String,
    #[serde(default)]
    pull_request: Option<Value>,
    labels: Vec<LabelDto>,
}

#[derive(Deserialize)]
struct ActorDto {
    id: u64,
}

#[derive(Deserialize)]
struct EventDto {
    id: u64,
    event: String,
    actor: Option<ActorDto>,
    label: Option<LabelDto>,
    created_at: String,
}

#[derive(Deserialize)]
struct UserDto {
    id: u64,
}

#[derive(Deserialize)]
struct HeadRepositoryDto {
    id: u64,
    full_name: String,
}

#[derive(Deserialize)]
struct PullHeadDto {
    #[serde(rename = "ref")]
    branch: String,
    sha: String,
    repo: Option<HeadRepositoryDto>,
}

#[derive(Deserialize)]
struct PullBaseDto {
    #[serde(rename = "ref")]
    branch: String,
    sha: String,
}

#[derive(Deserialize)]
struct PullRequestDto {
    id: u64,
    number: u64,
    state: String,
    draft: bool,
    merged: bool,
    mergeable: Option<bool>,
    mergeable_state: String,
    user: UserDto,
    head: PullHeadDto,
    base: PullBaseDto,
}

#[derive(Deserialize)]
struct AppDto {
    id: u64,
}

#[derive(Deserialize)]
struct CheckRunDto {
    id: u64,
    name: String,
    head_sha: String,
    status: CheckStatus,
    conclusion: Option<CheckConclusion>,
    started_at: Option<String>,
    completed_at: Option<String>,
    app: AppDto,
}

#[derive(Deserialize)]
struct CheckRunsPageDto {
    total_count: usize,
    check_runs: Vec<CheckRunDto>,
}

#[derive(Deserialize)]
struct CommitStatusDto {
    id: u64,
    context: String,
    state: CommitStatusState,
    creator: UserDto,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct CombinedStatusDto {
    sha: String,
    state: CommitStatusState,
    statuses: Vec<CommitStatusDto>,
}

#[derive(Deserialize)]
struct ReviewDto {
    id: u64,
    user: UserDto,
    state: ReviewState,
    commit_id: Option<String>,
    submitted_at: Option<String>,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct IssueCommentDto {
    id: u64,
    user: UserDto,
    issue_url: String,
    html_url: String,
    body: String,
    created_at: String,
    updated_at: String,
}

pub struct GitHubReader<T> {
    transport: T,
    base_url: String,
    token: String,
    max_response_bytes: usize,
    max_pages: usize,
}

impl<T: ReadTransport> GitHubReader<T> {
    pub fn new(
        transport: T,
        base_url: impl Into<String>,
        token: impl Into<String>,
        max_response_bytes: usize,
        max_pages: usize,
    ) -> Result<Self, GitHubError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let token = token.into();
        if !base_url.starts_with("https://")
            || token.trim().is_empty()
            || max_response_bytes == 0
            || max_pages == 0
        {
            return Err(GitHubError::InvalidConfiguration);
        }
        Ok(Self {
            transport,
            base_url,
            token,
            max_response_bytes,
            max_pages,
        })
    }

    pub fn discover_open_issues(
        &self,
        owner: &str,
        repository: &str,
        label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError> {
        if !valid_segment(owner) || !valid_segment(repository) || !valid_segment(label) {
            return Err(GitHubError::InvalidRepository);
        }
        let issues = self.get_pages::<IssueDto>(&format!(
            "/repos/{owner}/{repository}/issues?state=open&labels={label}&per_page=100&page=1"
        ))?;
        let mut ids = BTreeSet::new();
        let mut numbers = BTreeSet::new();
        issues
            .into_iter()
            .map(|issue| {
                let labels = issue
                    .labels
                    .into_iter()
                    .map(|label| label.name)
                    .collect::<BTreeSet<_>>();
                if issue.id == 0
                    || issue.number == 0
                    || issue.state != "open"
                    || !ids.insert(issue.id)
                    || !numbers.insert(issue.number)
                    || labels.iter().any(|label| label.trim().is_empty())
                    || !labels.contains(label)
                {
                    return Err(GitHubError::InvalidIdentity);
                }
                Ok(IssueSnapshot {
                    id: issue.id,
                    number: issue.number,
                    open: true,
                    is_pull_request: issue.pull_request.is_some(),
                    labels,
                })
            })
            .collect()
    }

    pub fn read_intake(
        &self,
        owner: &str,
        repository: &str,
        issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError> {
        if !valid_segment(owner) || !valid_segment(repository) {
            return Err(GitHubError::InvalidRepository);
        }
        if issue_number == 0 {
            return Err(GitHubError::InvalidIssueNumber);
        }
        let root = format!("/repos/{owner}/{repository}");
        let repository_dto: RepositoryDto = self.get_json(&root)?;
        let expected_name = format!("{owner}/{repository}");
        if repository_dto.id == 0
            || repository_dto.full_name != expected_name
            || repository_dto.default_branch.trim().is_empty()
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let issue_dto: IssueDto = self.get_json(&format!("{root}/issues/{issue_number}"))?;
        if issue_dto.id == 0
            || issue_dto.number != issue_number
            || !matches!(issue_dto.state.as_str(), "open" | "closed")
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let events = self.get_pages::<EventDto>(&format!(
            "{root}/issues/{issue_number}/events?per_page=100&page=1"
        ))?;
        let mut label_events = Vec::new();
        for event in events {
            if !matches!(event.event.as_str(), "labeled" | "unlabeled") {
                continue;
            }
            let actor = event.actor.ok_or(GitHubError::InvalidIdentity)?;
            let label = event.label.ok_or(GitHubError::InvalidIdentity)?;
            if event.id == 0
                || actor.id == 0
                || label.name.trim().is_empty()
                || event.created_at.trim().is_empty()
            {
                return Err(GitHubError::InvalidIdentity);
            }
            label_events.push(LabelEvent {
                id: event.id,
                labeled: event.event == "labeled",
                actor_id: actor.id,
                label: label.name,
                created_at: event.created_at,
            });
        }
        Ok(IntakeSnapshot {
            repository: RepositorySnapshot {
                id: repository_dto.id,
                full_name: repository_dto.full_name,
                default_branch: repository_dto.default_branch,
            },
            issue: IssueSnapshot {
                id: issue_dto.id,
                number: issue_dto.number,
                open: issue_dto.state == "open",
                is_pull_request: issue_dto.pull_request.is_some(),
                labels: issue_dto
                    .labels
                    .into_iter()
                    .map(|label| label.name)
                    .collect(),
            },
            label_events,
        })
    }

    pub fn read_pull_request(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError> {
        if !valid_segment(owner) || !valid_segment(repository) {
            return Err(GitHubError::InvalidRepository);
        }
        if repository_id == 0 || pull_request_number == 0 {
            return Err(GitHubError::InvalidIdentity);
        }
        let root = format!("/repos/{owner}/{repository}");
        let expected_repository = format!("{owner}/{repository}");
        let pull: PullRequestDto = self.get_json(&format!("{root}/pulls/{pull_request_number}"))?;
        let head_repository = pull.head.repo.ok_or(GitHubError::InvalidIdentity)?;
        if pull.id == 0
            || pull.number != pull_request_number
            || !matches!(pull.state.as_str(), "open" | "closed")
            || pull.user.id == 0
            || head_repository.id != repository_id
            || head_repository.full_name != expected_repository
            || pull.head.branch.trim().is_empty()
            || !valid_sha(&pull.head.sha)
            || pull.base.branch.trim().is_empty()
            || !valid_sha(&pull.base.sha)
            || pull.mergeable_state.trim().is_empty()
        {
            return Err(GitHubError::InvalidIdentity);
        }

        let check_dtos = self.get_check_pages(&format!(
            "{root}/commits/{}/check-runs?filter=all&per_page=100&page=1",
            pull.head.sha
        ))?;
        let mut check_ids = BTreeSet::new();
        let mut check_runs = Vec::with_capacity(check_dtos.len());
        for check in check_dtos {
            let conclusion_valid = match check.status {
                CheckStatus::Completed => check.conclusion.is_some(),
                _ => check.conclusion.is_none(),
            };
            if check.id == 0
                || check.app.id == 0
                || !check_ids.insert(check.id)
                || check.name.trim().is_empty()
                || check.head_sha != pull.head.sha
                || !conclusion_valid
            {
                return Err(GitHubError::InvalidIdentity);
            }
            check_runs.push(CheckRunSnapshot {
                id: check.id,
                app_id: check.app.id,
                name: check.name,
                head_sha: check.head_sha,
                status: check.status,
                conclusion: check.conclusion,
                started_at: check.started_at,
                completed_at: check.completed_at,
            });
        }

        let combined: CombinedStatusDto =
            self.get_json(&format!("{root}/commits/{}/status", pull.head.sha))?;
        if combined.sha != pull.head.sha {
            return Err(GitHubError::InvalidIdentity);
        }
        let mut status_ids = BTreeSet::new();
        let mut commit_statuses = Vec::with_capacity(combined.statuses.len());
        for status in combined.statuses {
            if status.id == 0
                || status.creator.id == 0
                || !status_ids.insert(status.id)
                || status.context.trim().is_empty()
                || status.created_at.trim().is_empty()
                || status.updated_at.trim().is_empty()
            {
                return Err(GitHubError::InvalidIdentity);
            }
            commit_statuses.push(CommitStatusSnapshot {
                id: status.id,
                creator_id: status.creator.id,
                context: status.context,
                state: status.state,
                created_at: status.created_at,
                updated_at: status.updated_at,
            });
        }

        let review_dtos = self.get_pages::<ReviewDto>(&format!(
            "{root}/pulls/{pull_request_number}/reviews?per_page=100&page=1"
        ))?;
        let mut review_ids = BTreeSet::new();
        let mut reviews = Vec::with_capacity(review_dtos.len());
        for review in review_dtos {
            if review.id == 0
                || review.user.id == 0
                || !review_ids.insert(review.id)
                || review
                    .commit_id
                    .as_deref()
                    .is_some_and(|sha| !valid_sha(sha))
            {
                return Err(GitHubError::InvalidIdentity);
            }
            let exact_head = review.commit_id.as_deref() == Some(pull.head.sha.as_str());
            reviews.push(ReviewSnapshot {
                id: review.id,
                actor_id: review.user.id,
                state: review.state,
                commit_id: review.commit_id,
                exact_head,
                submitted_at: review.submitted_at,
                body: review.body,
            });
        }

        Ok(PullRequestEvidence {
            pull_request: PullRequestSnapshot {
                id: pull.id,
                number: pull.number,
                open: pull.state == "open",
                draft: pull.draft,
                merged: pull.merged,
                mergeable: pull.mergeable,
                mergeable_state: pull.mergeable_state,
                author_id: pull.user.id,
                head_repository_id: head_repository.id,
                head_repository: head_repository.full_name,
                head_branch: pull.head.branch,
                head_sha: pull.head.sha,
                base_branch: pull.base.branch,
                base_sha: pull.base.sha,
            },
            check_runs,
            commit_status_state: combined.state,
            commit_statuses,
            reviews,
        })
    }

    pub fn read_issue_comment(
        &self,
        owner: &str,
        repository: &str,
        issue_number: u64,
        comment_id: u64,
    ) -> Result<IssueCommentSnapshot, GitHubError> {
        if !valid_segment(owner) || !valid_segment(repository) {
            return Err(GitHubError::InvalidRepository);
        }
        if issue_number == 0 || comment_id == 0 {
            return Err(GitHubError::InvalidIdentity);
        }
        let root = format!("/repos/{owner}/{repository}");
        let comment: IssueCommentDto =
            self.get_json(&format!("{root}/issues/comments/{comment_id}"))?;
        let expected_issue_url = format!("{}{root}/issues/{issue_number}", self.base_url);
        if comment.id != comment_id
            || comment.user.id == 0
            || comment.issue_url != expected_issue_url
            || comment.html_url.trim().is_empty()
            || comment.body.trim().is_empty()
            || comment.created_at.trim().is_empty()
            || comment.updated_at.trim().is_empty()
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let body_sha256 = hex_digest(&Sha256::digest(comment.body.as_bytes()));
        Ok(IssueCommentSnapshot {
            id: comment.id,
            actor_id: comment.user.id,
            issue_number,
            html_url: comment.html_url,
            body: comment.body,
            body_sha256,
            created_at: comment.created_at,
            updated_at: comment.updated_at,
        })
    }

    fn request(&self, path: &str) -> Result<ReadResponse, GitHubError> {
        if !path.starts_with('/') {
            return Err(GitHubError::UnsafePaginationUrl);
        }
        let response = self.transport.get(ReadRequest {
            method: "GET",
            url: format!("{}{}", self.base_url, path),
            headers: BTreeMap::from([
                ("accept".into(), "application/vnd.github+json".into()),
                ("authorization".into(), format!("Bearer {}", self.token)),
                ("user-agent".into(), "pip-control-plane".into()),
                ("x-github-api-version".into(), "2022-11-28".into()),
            ]),
            max_bytes: self.max_response_bytes,
        })?;
        if !(200..300).contains(&response.status) {
            return Err(GitHubError::HttpStatus(response.status));
        }
        if response.body.len() > self.max_response_bytes {
            return Err(GitHubError::ResponseTooLarge);
        }
        Ok(response)
    }

    fn get_json<D: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<D, GitHubError> {
        let response = self.request(path)?;
        serde_json::from_slice(&response.body)
            .map_err(|error| GitHubError::MalformedJson(error.to_string()))
    }

    fn get_pages<D: for<'de> Deserialize<'de>>(&self, first: &str) -> Result<Vec<D>, GitHubError> {
        let mut path = first.to_owned();
        let mut results = Vec::new();
        for page in 0..self.max_pages {
            let response = self.request(&path)?;
            let mut values: Vec<D> = serde_json::from_slice(&response.body)
                .map_err(|error| GitHubError::MalformedJson(error.to_string()))?;
            results.append(&mut values);
            let Some(link) = response.headers.get("link") else {
                return Ok(results);
            };
            let Some(next) = next_link(link) else {
                return Ok(results);
            };
            if page + 1 == self.max_pages {
                return Err(GitHubError::PaginationLimit);
            }
            path = self.safe_path(&next)?;
        }
        Err(GitHubError::PaginationLimit)
    }

    fn get_check_pages(&self, first: &str) -> Result<Vec<CheckRunDto>, GitHubError> {
        let mut path = first.to_owned();
        let mut results = Vec::new();
        let mut expected_total = None;
        for page in 0..self.max_pages {
            let response = self.request(&path)?;
            let payload: CheckRunsPageDto = serde_json::from_slice(&response.body)
                .map_err(|error| GitHubError::MalformedJson(error.to_string()))?;
            if expected_total
                .replace(payload.total_count)
                .is_some_and(|expected| expected != payload.total_count)
            {
                return Err(GitHubError::InvalidIdentity);
            }
            results.extend(payload.check_runs);
            let next = response
                .headers
                .get("link")
                .and_then(|link| next_link(link));
            match next {
                None if results.len() == payload.total_count => return Ok(results),
                None => return Err(GitHubError::InvalidIdentity),
                Some(_) if page + 1 == self.max_pages => {
                    return Err(GitHubError::PaginationLimit);
                }
                Some(next) => path = self.safe_path(&next)?,
            }
        }
        Err(GitHubError::PaginationLimit)
    }

    fn safe_path(&self, url: &str) -> Result<String, GitHubError> {
        if let Some(path) = url.strip_prefix(&self.base_url)
            && path.starts_with('/')
        {
            return Ok(path.to_owned());
        }
        if url.starts_with('/') {
            return Ok(url.to_owned());
        }
        Err(GitHubError::UnsafePaginationUrl)
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}

fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        if !part.contains("rel=\"next\"") {
            return None;
        }
        let start = part.find('<')? + 1;
        let end = part[start..].find('>')? + start;
        Some(part[start..end].to_owned())
    })
}

#[must_use]
pub fn verify_webhook(secret: &[u8], payload: &[u8], signature_header: &str) -> bool {
    let Some(signature) = signature_header.strip_prefix("sha256=") else {
        return false;
    };
    if signature.len() != 64 {
        return false;
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in signature.as_bytes().chunks_exact(2).enumerate() {
        let Some(high) = hex(pair[0]) else {
            return false;
        };
        let Some(low) = hex(pair[1]) else {
            return false;
        };
        bytes[index] = (high << 4) | low;
    }
    HmacSha256::new_from_slice(secret).is_ok_and(|mut mac| {
        mac.update(payload);
        mac.verify_slice(&bytes).is_ok()
    })
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
