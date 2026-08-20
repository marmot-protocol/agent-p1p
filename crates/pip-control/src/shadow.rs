//! Token-free, non-dispatching reconciliation over read-only evidence.

use std::fmt;

use pip_core::{ActorId, IntakeDecision, IssueObservation, evaluate_intake};
use pip_github::{GitHubError, GitHubReader, IntakeSnapshot, IssueSnapshot, ReadTransport};
use serde::Serialize;

use crate::RepositoryPolicy;

pub trait IntakeSource {
    fn discover(
        &self,
        owner: &str,
        repository: &str,
        label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError>;

    fn intake(
        &self,
        owner: &str,
        repository: &str,
        issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError>;
}

impl<T: ReadTransport> IntakeSource for GitHubReader<T> {
    fn discover(
        &self,
        owner: &str,
        repository: &str,
        label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError> {
        self.discover_open_issues(owner, repository, label)
    }

    fn intake(
        &self,
        owner: &str,
        repository: &str,
        issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError> {
        self.read_intake(owner, repository, issue_number)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ShadowCandidate {
    pub issue_number: u64,
    pub issue_id: u64,
    pub latest_label_actor_id: Option<u64>,
    pub decision: String,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ShadowReport {
    pub report_format: u32,
    pub observed_at: u64,
    pub repository_id: u64,
    pub repository: String,
    pub policy_revision: u64,
    pub intake_enabled: bool,
    pub dispatch_enabled: bool,
    pub mutation_count: u64,
    pub candidates: Vec<ShadowCandidate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShadowError {
    DispatchEnabled,
    Evidence(String),
    RepositoryDrift,
    DiscoveryDrift,
}

impl fmt::Display for ShadowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DispatchEnabled => {
                formatter.write_str("read-only shadow requires dispatch to be disabled")
            }
            Self::Evidence(error) => write!(formatter, "shadow evidence read failed: {error}"),
            Self::RepositoryDrift => {
                formatter.write_str("live repository identity differs from policy")
            }
            Self::DiscoveryDrift => formatter.write_str("discovery and issue evidence disagree"),
        }
    }
}

impl std::error::Error for ShadowError {}

pub fn reconcile_read_only<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    observed_at: u64,
    repository_active_cases: u32,
    global_active_cases: u32,
) -> Result<ShadowReport, ShadowError> {
    if policy.dispatch_enabled {
        return Err(ShadowError::DispatchEnabled);
    }
    let mut discovered = source
        .discover(
            &policy.repository.owner,
            &policy.repository.name,
            &policy.intake.label,
        )
        .map_err(|error| ShadowError::Evidence(error.to_string()))?;
    discovered.sort_by_key(|issue| issue.number);
    let intake_policy = policy.intake_policy(false);
    let mut candidates = Vec::with_capacity(discovered.len());
    for issue in discovered {
        let evidence = source
            .intake(
                &policy.repository.owner,
                &policy.repository.name,
                issue.number,
            )
            .map_err(|error| ShadowError::Evidence(error.to_string()))?;
        if evidence.repository.id != policy.repository.id
            || evidence.repository.full_name != policy.repository.full_name()
            || evidence.repository.default_branch != policy.repository.default_branch
        {
            return Err(ShadowError::RepositoryDrift);
        }
        if evidence.issue != issue {
            return Err(ShadowError::DiscoveryDrift);
        }
        let latest_label_actor_id = evidence
            .label_events
            .iter()
            .filter(|event| event.label == policy.intake.label)
            .max_by_key(|event| (&event.created_at, event.id))
            .filter(|event| event.labeled)
            .map(|event| event.actor_id);
        let observation = IssueObservation {
            open: evidence.issue.open,
            is_pull_request: evidence.issue.is_pull_request,
            labels: evidence.issue.labels.clone(),
            latest_label_actor_id: latest_label_actor_id
                .and_then(std::num::NonZeroU64::new)
                .map(ActorId::new),
            excluded: policy.intake.excluded_issue_numbers.contains(&issue.number),
            held: false,
            already_owned: false,
            repository_active_cases,
            global_active_cases,
        };
        let (decision, blockers) = match evaluate_intake(&intake_policy, &observation) {
            IntakeDecision::Eligible => ("ELIGIBLE".into(), Vec::new()),
            IntakeDecision::Ineligible(blockers) => (
                "INELIGIBLE".into(),
                blockers
                    .into_iter()
                    .map(|blocker| blocker.to_string())
                    .collect(),
            ),
        };
        candidates.push(ShadowCandidate {
            issue_number: issue.number,
            issue_id: issue.id,
            latest_label_actor_id,
            decision,
            blockers,
        });
    }
    Ok(ShadowReport {
        report_format: 1,
        observed_at,
        repository_id: policy.repository.id,
        repository: policy.repository.full_name(),
        policy_revision: policy.revision,
        intake_enabled: policy.intake.enabled,
        dispatch_enabled: policy.dispatch_enabled,
        mutation_count: 0,
        candidates,
    })
}
