//! Authenticated comment reads. Never follow URLs supplied by a webhook.
use super::*;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DiscussionComment {
    pub id: u64,
    pub actor_id: u64,
    pub human: bool,
    pub body: String,
    pub updated_at: String,
    pub reply_to: Option<u64>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub context: Value,
}

impl<T: ReadTransport> GitHubReader<T> {
    pub fn read_actor_login(&self, id: u64) -> Result<String, GitHubError> {
        if id == 0 {
            return Err(GitHubError::InvalidIdentity);
        }
        let user: Value = self.get_json(&format!("/user/{id}"))?;
        let login = user["login"]
            .as_str()
            .filter(|name| valid_segment(name))
            .ok_or(GitHubError::InvalidIdentity)?;
        if user["id"].as_u64() != Some(id) {
            return Err(GitHubError::InvalidIdentity);
        }
        Ok(login.into())
    }

    pub fn read_discussion_comment(
        &self,
        owner: &str,
        repository: &str,
        thread: u64,
        kind: &str,
        id: u64,
    ) -> Result<DiscussionComment, GitHubError> {
        if !valid_segment(owner) || !valid_segment(repository) || thread == 0 || id == 0 {
            return Err(GitHubError::InvalidIdentity);
        }
        let root = format!("/repos/{owner}/{repository}");
        let (path, parent_field, parent_path) = match kind {
            "issue_comment" => (
                format!("{root}/issues/comments/{id}"),
                "issue_url",
                format!("{root}/issues/{thread}"),
            ),
            "pull_request_review_comment" => (
                format!("{root}/pulls/comments/{id}"),
                "pull_request_url",
                format!("{root}/pulls/{thread}"),
            ),
            "pull_request_review" => (
                format!("{root}/pulls/{thread}/reviews/{id}"),
                "pull_request_url",
                format!("{root}/pulls/{thread}"),
            ),
            _ => return Err(GitHubError::InvalidIdentity),
        };
        let raw: Value = self.get_json(&path)?;
        if raw["id"].as_u64() != Some(id)
            || raw[parent_field].as_str()
                != Some(format!("{}{parent_path}", self.base_url).as_str())
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let actor_id = raw["user"]["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or(GitHubError::InvalidIdentity)?;
        let updated_at = raw["updated_at"]
            .as_str()
            .or_else(|| raw["submitted_at"].as_str())
            .filter(|s| !s.is_empty())
            .ok_or(GitHubError::InvalidIdentity)?;
        let body = raw["body"].as_str().unwrap_or_default();
        if body.len() > 64 * 1024 {
            return Err(GitHubError::InvalidIdentity);
        }
        let mut context = serde_json::Map::new();
        for field in [
            "path",
            "diff_hunk",
            "commit_id",
            "original_commit_id",
            "line",
            "original_line",
            "side",
            "state",
        ] {
            if let Some(value) = raw.get(field) {
                context.insert(field.into(), value.clone());
            }
        }
        let context = if context.is_empty() {
            Value::Null
        } else {
            Value::Object(context)
        };
        if serde_json::to_vec(&context)
            .map_err(|_| GitHubError::InvalidIdentity)?
            .len()
            > 32 * 1024
        {
            return Err(GitHubError::ResponseTooLarge);
        }
        Ok(DiscussionComment {
            id,
            actor_id,
            human: raw["user"]["type"] == "User",
            body: body.into(),
            updated_at: updated_at.into(),
            reply_to: raw["in_reply_to_id"].as_u64(),
            context,
        })
    }
}
