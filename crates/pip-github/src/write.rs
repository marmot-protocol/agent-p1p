//! Idempotent, provenance-marked GitHub mutations.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{GitHubError, ReadResponse, UreqTransport, next_link, valid_segment, valid_sha};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub max_bytes: usize,
}

pub trait MutationTransport {
    fn send(&self, request: MutationRequest) -> Result<ReadResponse, GitHubError>;
}

impl MutationTransport for UreqTransport {
    fn send(&self, request: MutationRequest) -> Result<ReadResponse, GitHubError> {
        let response = match request.method {
            "GET" => {
                let mut call = self.agent.get(&request.url);
                for (name, value) in &request.headers {
                    call = call.header(name, value);
                }
                call.call()
            }
            "POST" => {
                let mut call = self.agent.post(&request.url);
                for (name, value) in &request.headers {
                    call = call.header(name, value);
                }
                call.send(&request.body)
            }
            "PATCH" => {
                let mut call = self.agent.patch(&request.url);
                for (name, value) in &request.headers {
                    call = call.header(name, value);
                }
                call.send(&request.body)
            }
            "PUT" => {
                let mut call = self.agent.put(&request.url);
                for (name, value) in &request.headers {
                    call = call.header(name, value);
                }
                call.send(&request.body)
            }
            _ => return Err(GitHubError::InvalidMutation),
        }
        .map_err(|error| GitHubError::Transport(error.to_string()))?;
        let mut response = response;
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
pub enum MutationResult {
    Created(u64),
    Existing(u64),
    Updated(u64),
    Merged(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentSpec {
    pub owner: String,
    pub repository: String,
    pub issue_number: u64,
    pub effect_id: String,
    pub expected_actor_id: u64,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestSpec {
    pub owner: String,
    pub repository: String,
    pub repository_id: u64,
    pub effect_id: String,
    pub expected_actor_id: u64,
    pub title: String,
    pub body: String,
    pub head_branch: String,
    pub head_sha: String,
    pub base_branch: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestReadySpec {
    pub owner: String,
    pub repository: String,
    pub repository_id: u64,
    pub pull_request_number: u64,
    pub expected_actor_id: u64,
    pub expected_head_branch: String,
    pub expected_head_sha: String,
    pub expected_base_branch: String,
    pub client_mutation_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewEvent {
    fn request_value(self) -> &'static str {
        match self {
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
            Self::Comment => "COMMENT",
        }
    }

    fn response_state(self) -> &'static str {
        match self {
            Self::Approve => "APPROVED",
            Self::RequestChanges => "CHANGES_REQUESTED",
            Self::Comment => "COMMENTED",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewMutationSpec {
    pub owner: String,
    pub repository: String,
    pub pull_request_number: u64,
    pub effect_id: String,
    pub expected_actor_id: u64,
    pub expected_head_sha: String,
    pub body: String,
    pub event: ReviewEvent,
}

#[derive(Deserialize)]
struct UserDto {
    id: u64,
}

#[derive(Deserialize)]
struct CommentDto {
    id: u64,
    user: UserDto,
    body: String,
    html_url: String,
}

#[derive(Deserialize)]
struct RepositoryDto {
    id: u64,
}

#[derive(Deserialize)]
struct PullRequestHeadDto {
    r#ref: String,
    sha: String,
    repo: Option<RepositoryDto>,
}

#[derive(Deserialize)]
struct PullRequestBaseDto {
    r#ref: String,
}

#[derive(Deserialize)]
struct PullRequestDto {
    id: u64,
    #[serde(default)]
    node_id: String,
    number: u64,
    state: String,
    draft: bool,
    title: String,
    body: Option<String>,
    html_url: String,
    user: UserDto,
    head: PullRequestHeadDto,
    base: PullRequestBaseDto,
}

#[derive(Deserialize)]
struct ReadyGraphQlResponse {
    data: Option<ReadyGraphQlData>,
    #[serde(default)]
    errors: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyGraphQlData {
    #[serde(alias = "convertPullRequestToDraft")]
    mark_pull_request_ready_for_review: ReadyGraphQlPayload,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyGraphQlPayload {
    pull_request: ReadyGraphQlPullRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyGraphQlPullRequest {
    id: String,
    is_draft: bool,
}

#[derive(Deserialize)]
struct ReviewDto {
    id: u64,
    user: UserDto,
    body: Option<String>,
    state: String,
    commit_id: String,
    html_url: String,
}

pub struct GitHubWriter<T> {
    transport: T,
    base_url: String,
    token: String,
    max_response_bytes: usize,
    max_pages: usize,
}

impl<T: MutationTransport> GitHubWriter<T> {
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

    pub fn ensure_issue_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError> {
        validate_comment(spec)?;
        let marker = marker(&spec.effect_id, &spec.body);
        let full_body = format!("{marker}\n{}", spec.body);
        let effect_prefix = format!("<!-- pip-control:v1 effect={} ", spec.effect_id);
        let root = format!(
            "/repos/{}/{}/issues/{}",
            spec.owner, spec.repository, spec.issue_number
        );
        let comments =
            self.get_pages::<CommentDto>(&format!("{root}/comments?per_page=100&page=1"))?;
        let owned = comments
            .iter()
            .filter(|comment| comment.body.starts_with(&effect_prefix))
            .collect::<Vec<_>>();
        match owned.as_slice() {
            [] => {}
            [comment] if comment.user.id != spec.expected_actor_id => {
                return Err(GitHubError::OwnershipConflict);
            }
            [comment] if comment.body == full_body => {
                validate_comment_dto(comment)?;
                return Ok(MutationResult::Existing(comment.id));
            }
            [_] => return Err(GitHubError::IdempotencyConflict),
            _ => return Err(GitHubError::DuplicateOwnership),
        }
        let response: CommentDto = self.mutate_json(
            "POST",
            &format!("{root}/comments"),
            &json!({"body": full_body}),
        )?;
        validate_comment_dto(&response)?;
        if response.user.id != spec.expected_actor_id || response.body != full_body {
            return Err(GitHubError::OwnershipConflict);
        }
        Ok(MutationResult::Created(response.id))
    }

    pub fn render_pull_request_body(&self, spec: &PullRequestSpec) -> Result<String, GitHubError> {
        validate_pull_request(spec)?;
        let digest = digest_fields(&[
            &spec.title,
            &spec.body,
            &spec.head_branch,
            &spec.head_sha,
            &spec.base_branch,
        ]);
        Ok(format!(
            "{}\n{}",
            marker_with_digest(&spec.effect_id, &digest),
            spec.body
        ))
    }

    pub fn ensure_draft_pull_request(
        &self,
        spec: &PullRequestSpec,
    ) -> Result<MutationResult, GitHubError> {
        let full_body = self.render_pull_request_body(spec)?;
        let effect_prefix = effect_prefix(&spec.effect_id);
        let head = percent_encode_query(&format!("{}:{}", spec.owner, spec.head_branch));
        let root = format!("/repos/{}/{}/pulls", spec.owner, spec.repository);
        let pull_requests = self.get_pages::<PullRequestDto>(&format!(
            "{root}?state=all&head={head}&per_page=100&page=1"
        ))?;
        let owned = pull_requests
            .iter()
            .filter(|pull_request| {
                pull_request
                    .body
                    .as_deref()
                    .is_some_and(|body| body.starts_with(&effect_prefix))
            })
            .collect::<Vec<_>>();

        if pull_requests.len() > owned.len() {
            return Err(GitHubError::OwnershipConflict);
        }
        let existing = match owned.as_slice() {
            [] => None,
            [pull_request] => Some(*pull_request),
            _ => return Err(GitHubError::DuplicateOwnership),
        };

        if let Some(pull_request) = existing {
            validate_pull_request_identity(pull_request, spec)?;
            if pull_request.title == spec.title
                && pull_request.body.as_deref() == Some(full_body.as_str())
            {
                return Ok(MutationResult::Existing(pull_request.number));
            }
            let response: PullRequestDto = self.mutate_json(
                "PATCH",
                &format!("{root}/{}", pull_request.number),
                &json!({
                    "title": spec.title,
                    "body": full_body,
                    "base": spec.base_branch,
                }),
            )?;
            validate_pull_request_identity(&response, spec)?;
            validate_pull_request_content(&response, spec, &full_body)?;
            return Ok(MutationResult::Updated(response.number));
        }

        let response: PullRequestDto = self.mutate_json(
            "POST",
            &root,
            &json!({
                "title": spec.title,
                "body": full_body,
                "head": spec.head_branch,
                "base": spec.base_branch,
                "draft": true,
                "maintainer_can_modify": false,
            }),
        )?;
        validate_pull_request_identity(&response, spec)?;
        validate_pull_request_content(&response, spec, &full_body)?;
        Ok(MutationResult::Created(response.number))
    }

    pub fn render_review_body(&self, spec: &ReviewMutationSpec) -> Result<String, GitHubError> {
        validate_review(spec)?;
        let digest = digest_fields(&[
            &spec.pull_request_number.to_string(),
            &spec.expected_head_sha,
            spec.event.request_value(),
            &spec.body,
        ]);
        Ok(format!(
            "{}\n{}",
            marker_with_digest(&spec.effect_id, &digest),
            spec.body
        ))
    }

    pub fn mark_pull_request_ready(
        &self,
        spec: &PullRequestReadySpec,
    ) -> Result<MutationResult, GitHubError> {
        self.set_pull_request_draft(spec, false)
    }

    pub fn mark_pull_request_draft(
        &self,
        spec: &PullRequestReadySpec,
    ) -> Result<MutationResult, GitHubError> {
        self.set_pull_request_draft(spec, true)
    }

    fn set_pull_request_draft(
        &self,
        spec: &PullRequestReadySpec,
        draft: bool,
    ) -> Result<MutationResult, GitHubError> {
        validate_ready_spec(spec)?;
        let root = format!(
            "/repos/{}/{}/pulls/{}",
            spec.owner, spec.repository, spec.pull_request_number
        );
        let before: PullRequestDto = self.mutate_json("GET", &root, &json!({}))?;
        validate_ready_identity(&before, spec)?;
        if before.draft == draft {
            return Ok(MutationResult::Existing(before.number));
        }
        if before.node_id.trim().is_empty() {
            return Err(GitHubError::InvalidIdentity);
        }
        let response: ReadyGraphQlResponse = self.mutate_json(
            "POST",
            "/graphql",
            &json!({
                "query": if draft {
                    "mutation DraftPipFollowUp($input: ConvertPullRequestToDraftInput!) { convertPullRequestToDraft(input: $input) { pullRequest { id isDraft } } }"
                } else {
                    "mutation MarkPipPullRequestReady($input: MarkPullRequestReadyForReviewInput!) { markPullRequestReadyForReview(input: $input) { pullRequest { id isDraft } } }"
                },
                "variables": {"input": {
                    "pullRequestId": before.node_id,
                    "clientMutationId": spec.client_mutation_id,
                }},
            }),
        )?;
        if !response.errors.is_empty() {
            return Err(GitHubError::InvalidMutation);
        }
        let updated = response.data.ok_or(GitHubError::InvalidIdentity)?;
        if updated.mark_pull_request_ready_for_review.pull_request.id != before.node_id
            || updated
                .mark_pull_request_ready_for_review
                .pull_request
                .is_draft
                != draft
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let after: PullRequestDto = self.mutate_json("GET", &root, &json!({}))?;
        validate_ready_identity(&after, spec)?;
        if after.draft != draft {
            return Err(GitHubError::InvalidIdentity);
        }
        Ok(MutationResult::Updated(after.number))
    }

    pub fn ensure_pull_request_review(
        &self,
        spec: &ReviewMutationSpec,
    ) -> Result<MutationResult, GitHubError> {
        let full_body = self.render_review_body(spec)?;
        let effect_prefix = effect_prefix(&spec.effect_id);
        let root = format!(
            "/repos/{}/{}/pulls/{}/reviews",
            spec.owner, spec.repository, spec.pull_request_number
        );
        let reviews = self.get_pages::<ReviewDto>(&format!("{root}?per_page=100&page=1"))?;
        let owned = reviews
            .iter()
            .filter(|review| {
                review
                    .body
                    .as_deref()
                    .is_some_and(|body| body.starts_with(&effect_prefix))
            })
            .collect::<Vec<_>>();
        match owned.as_slice() {
            [] => {}
            [review] if review.user.id != spec.expected_actor_id => {
                return Err(GitHubError::OwnershipConflict);
            }
            [review]
                if review.body.as_deref() == Some(full_body.as_str())
                    && review.state == spec.event.response_state()
                    && review.commit_id == spec.expected_head_sha =>
            {
                validate_review_dto(review)?;
                return Ok(MutationResult::Existing(review.id));
            }
            [_] => return Err(GitHubError::IdempotencyConflict),
            _ => return Err(GitHubError::DuplicateOwnership),
        }

        let response: ReviewDto = self.mutate_json(
            "POST",
            &root,
            &json!({
                "commit_id": spec.expected_head_sha,
                "body": full_body,
                "event": spec.event.request_value(),
            }),
        )?;
        validate_review_dto(&response)?;
        if response.user.id != spec.expected_actor_id
            || response.body.as_deref() != Some(full_body.as_str())
            || response.state != spec.event.response_state()
            || response.commit_id != spec.expected_head_sha
        {
            return Err(GitHubError::OwnershipConflict);
        }
        Ok(MutationResult::Created(response.id))
    }

    fn get_pages<D: for<'de> Deserialize<'de>>(&self, first: &str) -> Result<Vec<D>, GitHubError> {
        let mut path = first.to_owned();
        let mut results = Vec::new();
        for page in 0..self.max_pages {
            let response = self.request("GET", &path, Vec::new())?;
            let mut values: Vec<D> = serde_json::from_slice(&response.body)
                .map_err(|error| GitHubError::MalformedJson(error.to_string()))?;
            results.append(&mut values);
            let next = response
                .headers
                .get("link")
                .and_then(|link| next_link(link));
            match next {
                None => return Ok(results),
                Some(_) if page + 1 == self.max_pages => {
                    return Err(GitHubError::PaginationLimit);
                }
                Some(next) => path = self.safe_path(&next)?,
            }
        }
        Err(GitHubError::PaginationLimit)
    }

    fn mutate_json<D: for<'de> Deserialize<'de>>(
        &self,
        method: &'static str,
        path: &str,
        body: &Value,
    ) -> Result<D, GitHubError> {
        let body = serde_json::to_vec(body)
            .map_err(|error| GitHubError::MalformedJson(error.to_string()))?;
        let response = self.request(method, path, body)?;
        serde_json::from_slice(&response.body)
            .map_err(|error| GitHubError::MalformedJson(error.to_string()))
    }

    fn request(
        &self,
        method: &'static str,
        path: &str,
        body: Vec<u8>,
    ) -> Result<ReadResponse, GitHubError> {
        if !path.starts_with('/') {
            return Err(GitHubError::InvalidMutation);
        }
        let response = self.transport.send(MutationRequest {
            method,
            url: format!("{}{}", self.base_url, path),
            headers: BTreeMap::from([
                ("accept".into(), "application/vnd.github+json".into()),
                ("authorization".into(), format!("Bearer {}", self.token)),
                ("content-type".into(), "application/json".into()),
                ("user-agent".into(), "pip-control-plane".into()),
                ("x-github-api-version".into(), "2022-11-28".into()),
            ]),
            body,
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

    fn safe_path(&self, url: &str) -> Result<String, GitHubError> {
        if let Some(path) = url.strip_prefix(&self.base_url)
            && path.starts_with('/')
        {
            return Ok(path.into());
        }
        if url.starts_with('/') {
            return Ok(url.into());
        }
        Err(GitHubError::UnsafePaginationUrl)
    }
}

fn validate_comment(spec: &CommentSpec) -> Result<(), GitHubError> {
    if !valid_segment(&spec.owner)
        || !valid_segment(&spec.repository)
        || spec.issue_number == 0
        || spec.expected_actor_id == 0
        || !valid_effect_id(&spec.effect_id)
        || spec.body.trim().is_empty()
        || spec.body.len() > 60_000
    {
        return Err(GitHubError::InvalidMutation);
    }
    Ok(())
}

fn validate_comment_dto(comment: &CommentDto) -> Result<(), GitHubError> {
    if comment.id == 0
        || comment.user.id == 0
        || comment.body.trim().is_empty()
        || comment.html_url.trim().is_empty()
    {
        return Err(GitHubError::InvalidIdentity);
    }
    Ok(())
}

fn validate_pull_request(spec: &PullRequestSpec) -> Result<(), GitHubError> {
    if !valid_segment(&spec.owner)
        || !valid_segment(&spec.repository)
        || spec.repository_id == 0
        || spec.expected_actor_id == 0
        || !valid_effect_id(&spec.effect_id)
        || spec.title.trim().is_empty()
        || spec.title.len() > 256
        || spec.body.trim().is_empty()
        || spec.body.len() > 60_000
        || !spec.head_branch.starts_with("pip/")
        || !valid_git_ref(&spec.head_branch)
        || !valid_git_ref(&spec.base_branch)
        || !valid_sha(&spec.head_sha)
    {
        return Err(GitHubError::InvalidMutation);
    }
    Ok(())
}

fn validate_ready_spec(spec: &PullRequestReadySpec) -> Result<(), GitHubError> {
    if !valid_segment(&spec.owner)
        || !valid_segment(&spec.repository)
        || spec.repository_id == 0
        || spec.pull_request_number == 0
        || spec.expected_actor_id == 0
        || !valid_git_ref(&spec.expected_head_branch)
        || !valid_sha(&spec.expected_head_sha)
        || !valid_git_ref(&spec.expected_base_branch)
        || !valid_effect_id(&spec.client_mutation_id)
    {
        return Err(GitHubError::InvalidMutation);
    }
    Ok(())
}

fn validate_ready_identity(
    pull_request: &PullRequestDto,
    spec: &PullRequestReadySpec,
) -> Result<(), GitHubError> {
    let valid = pull_request.id != 0
        && pull_request.number == spec.pull_request_number
        && pull_request.state == "open"
        && pull_request.user.id == spec.expected_actor_id
        && pull_request.head.r#ref == spec.expected_head_branch
        && pull_request.head.sha == spec.expected_head_sha
        && pull_request.head.repo.as_ref().map(|repo| repo.id) == Some(spec.repository_id)
        && pull_request.base.r#ref == spec.expected_base_branch
        && !pull_request.html_url.trim().is_empty();
    if valid {
        Ok(())
    } else {
        Err(GitHubError::OwnershipConflict)
    }
}

fn validate_pull_request_identity(
    pull_request: &PullRequestDto,
    spec: &PullRequestSpec,
) -> Result<(), GitHubError> {
    let valid = pull_request.id != 0
        && pull_request.number != 0
        && pull_request.state == "open"
        && pull_request.draft
        && pull_request.user.id == spec.expected_actor_id
        && pull_request.head.r#ref == spec.head_branch
        && pull_request.head.sha == spec.head_sha
        && pull_request.head.repo.as_ref().map(|repo| repo.id) == Some(spec.repository_id)
        && pull_request.base.r#ref == spec.base_branch
        && !pull_request.html_url.trim().is_empty();
    if valid {
        Ok(())
    } else {
        Err(GitHubError::OwnershipConflict)
    }
}

fn validate_pull_request_content(
    pull_request: &PullRequestDto,
    spec: &PullRequestSpec,
    full_body: &str,
) -> Result<(), GitHubError> {
    if pull_request.title == spec.title && pull_request.body.as_deref() == Some(full_body) {
        Ok(())
    } else {
        Err(GitHubError::IdempotencyConflict)
    }
}

fn validate_review(spec: &ReviewMutationSpec) -> Result<(), GitHubError> {
    if !valid_segment(&spec.owner)
        || !valid_segment(&spec.repository)
        || spec.pull_request_number == 0
        || spec.expected_actor_id == 0
        || !valid_effect_id(&spec.effect_id)
        || !valid_sha(&spec.expected_head_sha)
        || spec.body.trim().is_empty()
        || spec.body.len() > 60_000
    {
        return Err(GitHubError::InvalidMutation);
    }
    Ok(())
}

fn validate_review_dto(review: &ReviewDto) -> Result<(), GitHubError> {
    if review.id == 0
        || review.user.id == 0
        || review.body.as_deref().is_none_or(str::is_empty)
        || !valid_sha(&review.commit_id)
        || review.html_url.trim().is_empty()
    {
        return Err(GitHubError::InvalidIdentity);
    }
    Ok(())
}

fn valid_git_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.ends_with('.')
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains("@{")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
}

fn valid_effect_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'#' | b'@')
        })
}

fn marker(effect_id: &str, body: &str) -> String {
    let digest = hex_digest(&Sha256::digest(body.as_bytes()));
    marker_with_digest(effect_id, &digest)
}

fn marker_with_digest(effect_id: &str, digest: &str) -> String {
    format!("<!-- pip-control:v1 effect={effect_id} sha256={digest} -->")
}

fn effect_prefix(effect_id: &str) -> String {
    format!("<!-- pip-control:v1 effect={effect_id} ")
}

fn digest_fields(fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field.len().to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hex_digest(&hasher.finalize())
}

fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(b"0123456789ABCDEF"[usize::from(byte >> 4)]));
            encoded.push(char::from(b"0123456789ABCDEF"[usize::from(byte & 0x0f)]));
        }
    }
    encoded
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
