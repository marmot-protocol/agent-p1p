//! Bounded, exact-head failure evidence. Remote text is data, never authority.
use super::*;
use serde_json::json;

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
        if check["id"].as_u64() != Some(check_id) || check["head_sha"] != head {
            return Err(GitHubError::InvalidIdentity);
        }
        // Do not follow an arbitrary details_url supplied by a check author.
        let prefix = format!("https://github.com/{owner}/{repository}/actions/runs/");
        let parts = check["details_url"]
            .as_str()
            .and_then(|url| url.strip_prefix(&prefix))
            .map(|tail| tail.split('/').collect::<Vec<_>>())
            .ok_or(GitHubError::InvalidIdentity)?;
        if parts.len() != 3 || parts[1] != "job" {
            return Err(GitHubError::InvalidIdentity);
        }
        let number = |value: &str| {
            value
                .parse::<u64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or(GitHubError::InvalidIdentity)
        };
        let run_id = number(parts[0])?;
        let job_id = number(parts[2])?;
        let job: Value = self.get_json(&format!("{root}/actions/jobs/{job_id}"))?;
        if job["id"].as_u64() != Some(job_id)
            || job["run_id"].as_u64() != Some(run_id)
            || job["head_sha"] != head
            || job["check_run_url"] != format!("{}{}", self.base_url, check_path)
        {
            return Err(GitHubError::InvalidIdentity);
        }
        // Ureq's default redirect policy never forwards Authorization. The
        // existing response-byte limit and timeout also apply to log downloads.
        let log = self.request(&format!("{root}/actions/jobs/{job_id}/logs"))?;
        let text = String::from_utf8_lossy(&log.body);
        let start = text.len().saturating_sub(16 * 1024);
        let start = (start..=text.len())
            .find(|n| text.is_char_boundary(*n))
            .unwrap_or(text.len());
        Ok(json!({
            "check_id":check_id,"head_sha":head,"run_id":run_id,"job_id":job_id,
            "details_url":check["details_url"],
            "log_sha256":hex_digest(&Sha256::digest(&log.body)),"log_bytes":log.body.len(),
            "log_excerpt":&text[start..],"log_truncated":start>0,
            "untrusted":true
        }))
    }
}
