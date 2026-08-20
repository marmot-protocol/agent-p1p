//! Fresh authorization gate for every active case before task release.

use std::fmt;

use pip_store::{Store, StoreError};
use serde::Serialize;

use crate::{IntakeSource, RepositoryPolicy, ShadowError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizationBlock {
    pub case_key: String,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ActiveAuthorization {
    Authorized { case_count: usize },
    Blocked { cases: Vec<AuthorizationBlock> },
}

impl ActiveAuthorization {
    #[must_use]
    pub const fn is_authorized(&self) -> bool {
        matches!(self, Self::Authorized { .. })
    }
}

#[derive(Debug)]
pub enum AuthorizationError {
    Evidence(ShadowError),
    Store(StoreError),
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AuthorizationError {}

impl From<StoreError> for AuthorizationError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub fn verify_active_authorization<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &Store,
) -> Result<ActiveAuthorization, AuthorizationError> {
    let mut cases = store
        .status(0)?
        .cases
        .into_iter()
        .filter(|case| {
            case.repository_id == policy.repository.id
                && !matches!(
                    case.state.as_str(),
                    "COMPLETED" | "ABANDONED" | "TAKEN_OVER"
                )
        })
        .collect::<Vec<_>>();
    cases.sort_by_key(|case| (case.issue_number, case.workflow_version));
    let case_count = cases.len();
    let mut blocked = Vec::new();
    for case in cases {
        let evidence = source
            .intake(
                &policy.repository.owner,
                &policy.repository.name,
                case.issue_number,
            )
            .map_err(|error| {
                AuthorizationError::Evidence(ShadowError::Evidence(error.to_string()))
            })?;
        let mut blockers = Vec::new();
        if case.policy_revision != policy.revision {
            blockers.push("POLICY_REVISION_MISMATCH".into());
        }
        if evidence.repository.id != policy.repository.id
            || evidence.repository.full_name != policy.repository.full_name()
            || evidence.repository.default_branch != policy.repository.default_branch
        {
            blockers.push("REPOSITORY_IDENTITY_MISMATCH".into());
        }
        if evidence.issue.number != case.issue_number {
            blockers.push("ISSUE_IDENTITY_MISMATCH".into());
        }
        if !evidence.issue.open {
            blockers.push("ISSUE_CLOSED".into());
        }
        if evidence.issue.is_pull_request {
            blockers.push("ISSUE_BECAME_PULL_REQUEST".into());
        }
        if policy
            .intake
            .excluded_issue_numbers
            .contains(&case.issue_number)
        {
            blockers.push("ISSUE_EXCLUDED".into());
        }
        let label_present = evidence.issue.labels.contains(&policy.intake.label);
        if !label_present {
            blockers.push("REQUIRED_LABEL_MISSING".into());
        }
        let latest = evidence
            .label_events
            .iter()
            .filter(|event| event.label == policy.intake.label)
            .max_by_key(|event| (&event.created_at, event.id));
        match latest {
            None => blockers.push("AUTHORIZATION_EVENT_MISSING".into()),
            Some(event) if !event.labeled => {
                blockers.push("LATEST_AUTHORIZATION_REMOVED".into());
            }
            Some(event) if !policy.intake.trusted_actor_ids.contains(&event.actor_id) => {
                blockers.push("UNTRUSTED_LABEL_ACTOR".into());
            }
            Some(_) => {}
        }
        if !blockers.is_empty() {
            blocked.push(AuthorizationBlock {
                case_key: case.case_key,
                blockers,
            });
        }
    }
    if blocked.is_empty() {
        Ok(ActiveAuthorization::Authorized { case_count })
    } else {
        Ok(ActiveAuthorization::Blocked { cases: blocked })
    }
}
