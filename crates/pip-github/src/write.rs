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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MergeModePolicy {
    pub guarded: bool,
    pub autonomous_merge: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeSpec {
    pub owner: String,
    pub repository: String,
    pub pull_request_number: u64,
    pub expected_head_sha: String,
    pub commit_title: String,
    pub method: String,
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
struct MergeDto {
    merged: bool,
    sha: Option<String>,
    #[allow(dead_code)]
    message: String,
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

    pub fn merge_pull_request(
        &self,
        spec: &MergeSpec,
        policy: MergeModePolicy,
    ) -> Result<MutationResult, GitHubError> {
        if !policy.guarded || !policy.autonomous_merge {
            return Err(GitHubError::MutationDisabled);
        }
        if !valid_segment(&spec.owner)
            || !valid_segment(&spec.repository)
            || spec.pull_request_number == 0
            || !valid_sha(&spec.expected_head_sha)
            || spec.commit_title.trim().is_empty()
            || !matches!(spec.method.as_str(), "merge" | "squash" | "rebase")
        {
            return Err(GitHubError::InvalidMutation);
        }
        let response: MergeDto = self.mutate_json(
            "PUT",
            &format!(
                "/repos/{}/{}/pulls/{}/merge",
                spec.owner, spec.repository, spec.pull_request_number
            ),
            &json!({
                "sha": spec.expected_head_sha,
                "commit_title": spec.commit_title,
                "merge_method": spec.method,
            }),
        )?;
        let sha = response.sha.ok_or(GitHubError::InvalidIdentity)?;
        if !response.merged || !valid_sha(&sha) {
            return Err(GitHubError::InvalidIdentity);
        }
        Ok(MutationResult::Merged(sha))
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

fn valid_effect_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'#' | b'@')
        })
}

fn marker(effect_id: &str, body: &str) -> String {
    let digest = hex_digest(&Sha256::digest(body.as_bytes()));
    format!("<!-- pip-control:v1 effect={effect_id} sha256={digest} -->")
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
