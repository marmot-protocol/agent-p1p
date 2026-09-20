//! Additive, verified issue assignment. Never replace another person's assignment.
use super::*;

impl<T: ReadTransport> GitHubReader<T> {
    pub fn ensure_issue_assignment(
        &self,
        owner: &str,
        repository: &str,
        expected: &IssueSnapshot,
        actor_id: u64,
    ) -> Result<IssueSnapshot, GitHubError> {
        if !valid_segment(owner)
            || !valid_segment(repository)
            || expected.id == 0
            || expected.number == 0
            || actor_id == 0
            || !expected.open
            || expected.is_pull_request
            || expected.assigned_to_other(Some(actor_id))
        {
            return Err(GitHubError::InvalidIdentity);
        }
        let path = format!("/repos/{owner}/{repository}/issues/{}", expected.number);
        let current: IssueDto = self.get_json(&path)?;
        let current = validate(current, expected, actor_id)?;
        if current.assignee_ids.contains(&actor_id) {
            return Ok(current);
        }
        let login = self.read_actor_login(actor_id)?;
        // POST adds an assignee; PATCH with an assignee list could remove a
        // human who raced this read. Never overwrite that person's assignment.
        let response = self.post_json(
            &format!("{path}/assignees"),
            json_bytes(&serde_json::json!({"assignees":[login]}))?,
        )?;
        let updated: IssueDto = serde_json::from_slice(&response.body)
            .map_err(|e| GitHubError::MalformedJson(e.to_string()))?;
        let updated = validate(updated, expected, actor_id)?;
        // GitHub can silently ignore users who cannot be assigned. A 2xx alone
        // is not proof of ownership. Replaying after a lost response is safe.
        if !updated.assignee_ids.contains(&actor_id) {
            return Err(GitHubError::OwnershipConflict);
        }
        Ok(updated)
    }
}

fn validate(
    issue: IssueDto,
    expected: &IssueSnapshot,
    actor: u64,
) -> Result<IssueSnapshot, GitHubError> {
    let assignee_ids = issue.assignee_ids()?;
    let observed = IssueSnapshot {
        id: issue.id,
        number: issue.number,
        open: issue.state == "open",
        is_pull_request: issue.pull_request.is_some(),
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        assignee_ids,
    };
    let mut comparable = observed.clone();
    comparable.assignee_ids = expected.assignee_ids.clone();
    if comparable != *expected || observed.assigned_to_other(Some(actor)) {
        return Err(GitHubError::OwnershipConflict);
    }
    Ok(observed)
}
