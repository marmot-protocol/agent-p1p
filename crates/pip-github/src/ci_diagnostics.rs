//! Bounded, exact-head failure evidence. Remote text is data, never authority.
use super::*;
use serde_json::json;

impl CheckRunSnapshot {
    pub fn has_actions_job(&self, owner: &str, repository: &str) -> bool {
        self.details_url
            .as_deref()
            .and_then(|url| actions_job(url, owner, repository))
            .is_some()
    }
}

fn actions_job(url: &str, owner: &str, repository: &str) -> Option<(u64, u64)> {
    let prefix = format!("https://github.com/{owner}/{repository}/actions/runs/");
    let parts = url.strip_prefix(&prefix)?.split('/').collect::<Vec<_>>();
    if parts.len() != 3 || parts[1] != "job" {
        return None;
    }
    let number = |value: &str| value.parse::<u64>().ok().filter(|id| *id > 0);
    Some((number(parts[0])?, number(parts[2])?))
}

fn bounded_text(value: &Value) -> String {
    let text = value.as_str().unwrap_or_default();
    let mut end = text.len().min(4096);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

impl<T: ReadTransport> GitHubReader<T> {
    /// Fetch an Actions log only after binding its check, job, run and head.
    /// Non-Actions checks and inaccessible/oversized logs remain unavailable;
    /// callers must never turn diagnostic availability into CI acceptance.
    pub fn read_check_failure(
        &self,
        owner: &str,
        repository: &str,
        check_id: u64,
        head: &str,
    ) -> Result<Value, GitHubError> {
        if !valid_segment(owner) || !valid_segment(repository) || check_id == 0 || !valid_sha(head)
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let root = format!("/repos/{owner}/{repository}");
        let check_path = format!("{root}/check-runs/{check_id}");
        let check: Value = self.get_json(&check_path)?;
        let conclusion =
            serde_json::from_value::<CheckConclusion>(check["conclusion"].clone()).ok();
        if check["id"].as_u64() != Some(check_id)
            || check["head_sha"] != head
            || check["status"] != "completed"
            || !conclusion.is_some_and(CheckConclusion::is_failure)
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let mut result = json!({"check_id":check_id,"head_sha":head,
            "summary":bounded_text(&check["output"]["summary"]),
            "text":bounded_text(&check["output"]["text"]),
            "availability":"summary_only","untrusted":true});
        // Do not follow an arbitrary details_url supplied by a check author.
        let Some((run_id, job_id)) = check["details_url"]
            .as_str()
            .and_then(|url| actions_job(url, owner, repository))
        else {
            return Ok(result);
        };
        let Ok(job): Result<Value, _> = self.get_json(&format!("{root}/actions/jobs/{job_id}"))
        else {
            return Ok(result);
        };
        if job["id"].as_u64() != Some(job_id)
            || job["run_id"].as_u64() != Some(run_id)
            || job["head_sha"] != head
            || job["check_run_url"] != format!("{}{}", self.base_url, check_path)
        {
            return Err(GitHubError::InvalidIdentity);
        }
        // Ureq's default redirect policy never forwards Authorization. The
        // existing response-byte limit and timeout also apply to log downloads.
        let Ok(log) = self.request(&format!("{root}/actions/jobs/{job_id}/logs")) else {
            return Ok(result);
        };
        let text = String::from_utf8_lossy(&log.body);
        let start = text.len().saturating_sub(16 * 1024);
        let start = (start..=text.len())
            .find(|n| text.is_char_boundary(*n))
            .unwrap_or(text.len());
        result.as_object_mut().unwrap().extend(
            json!({
                "check_id":check_id,"head_sha":head,"run_id":run_id,"job_id":job_id,
                "details_url":check["details_url"],
                "log_sha256":hex_digest(&Sha256::digest(&log.body)),"log_bytes":log.body.len(),
                "log_excerpt":&text[start..],"log_truncated":start>0,
                "availability":"log_and_summary"
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        Ok(result)
    }
}
