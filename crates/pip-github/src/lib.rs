//! Read-only GitHub evidence adapter.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;

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
            Self::Transport(error) => write!(formatter, "GitHub transport failed: {error}"),
        }
    }
}

impl std::error::Error for GitHubError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositorySnapshot {
    pub id: u64,
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueSnapshot {
    pub id: u64,
    pub number: u64,
    pub open: bool,
    pub is_pull_request: bool,
    pub labels: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelEvent {
    pub id: u64,
    pub labeled: bool,
    pub actor_id: u64,
    pub label: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeSnapshot {
    pub repository: RepositorySnapshot,
    pub issue: IssueSnapshot,
    pub label_events: Vec<LabelEvent>,
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
