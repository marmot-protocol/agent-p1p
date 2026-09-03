//! Short-lived GitHub App installation authentication.

use std::collections::BTreeMap;
use std::fmt;

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{GitHubError, ReadRequest, ReadTransport};

const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_EXPIRY_BYTES: usize = 128;

pub struct GitHubAppCredentials<'a> {
    pub app_id: u64,
    pub installation_id: u64,
    pub repository_id: u64,
    pub private_key_pem: &'a [u8],
}

#[derive(Clone, Eq, PartialEq)]
pub struct InstallationToken {
    pub token: String,
    pub expires_at: String,
}

impl fmt::Debug for InstallationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstallationToken")
            .field("token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Serialize)]
struct AppClaims {
    iss: String,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
    expires_at: String,
}

pub fn mint_installation_token<T: ReadTransport>(
    transport: &T,
    base_url: &str,
    credentials: &GitHubAppCredentials<'_>,
    now: u64,
) -> Result<InstallationToken, GitHubError> {
    let base_url = base_url.trim_end_matches('/');
    if !base_url.starts_with("https://")
        || base_url.bytes().any(|byte| byte.is_ascii_whitespace())
        || credentials.app_id == 0
        || credentials.installation_id == 0
        || credentials.repository_id == 0
        || now < 60
    {
        return Err(GitHubError::InvalidConfiguration);
    }
    let expires_at = now
        .checked_add(9 * 60)
        .ok_or(GitHubError::InvalidConfiguration)?;
    let key = EncodingKey::from_rsa_pem(credentials.private_key_pem)
        .map_err(|_| GitHubError::InvalidConfiguration)?;
    let jwt = encode(
        &Header::new(Algorithm::RS256),
        &AppClaims {
            iss: credentials.app_id.to_string(),
            iat: now - 60,
            exp: expires_at,
        },
        &key,
    )
    .map_err(|_| GitHubError::InvalidConfiguration)?;
    let request = ReadRequest {
        method: "POST",
        url: format!(
            "{base_url}/app/installations/{}/access_tokens",
            credentials.installation_id
        ),
        headers: BTreeMap::from([
            ("accept".into(), "application/vnd.github+json".into()),
            ("authorization".into(), format!("Bearer {jwt}")),
            ("content-type".into(), "application/json".into()),
            ("x-github-api-version".into(), "2022-11-28".into()),
        ]),
        body: serde_json::to_vec(&json!({
            "repository_ids": [credentials.repository_id],
        }))
        .map_err(|error| GitHubError::MalformedJson(error.to_string()))?,
        max_bytes: MAX_TOKEN_RESPONSE_BYTES,
    };
    let response = transport.post(request)?;
    if response.status != 201 {
        return Err(GitHubError::HttpStatus(response.status));
    }
    if response.body.len() > MAX_TOKEN_RESPONSE_BYTES {
        return Err(GitHubError::ResponseTooLarge);
    }
    let response: TokenResponse = serde_json::from_slice(&response.body)
        .map_err(|error| GitHubError::MalformedJson(error.to_string()))?;
    if !valid_secret(&response.token, MAX_TOKEN_BYTES) || !valid_expiry(&response.expires_at) {
        return Err(GitHubError::InvalidIdentity);
    }
    Ok(InstallationToken {
        token: response.token,
        expires_at: response.expires_at,
    })
}

fn valid_secret(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
}

fn valid_expiry(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_EXPIRY_BYTES
        && value.is_ascii()
        && value.bytes().all(|byte| !byte.is_ascii_whitespace())
}
