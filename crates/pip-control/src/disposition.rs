//! Durable controller-owned human disposition and local terminal effects.

use std::fmt;

use pip_github::{
    CommentSpec, GitHubError, GitHubWriter, MutationResult, MutationTransport, PullRequestReadySpec,
};
use pip_store::{ApplyResult, EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;

const LOCAL_EFFECTS: [&str; 4] = [
    "RECORD_COMPLETION",
    "RECORD_ABANDONMENT",
    "RECORD_BLOCK",
    "RECORD_TAKEOVER",
];
const COMMENT_EFFECTS: [&str; 3] = ["HOLD_FOR_HUMAN", "NOTIFY_SHADOW_READY", "ESCALATE"];

pub trait DispositionWriter {
    fn ensure_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError>;
    fn mark_ready(&self, spec: &PullRequestReadySpec) -> Result<MutationResult, GitHubError>;
}

impl<T: MutationTransport> DispositionWriter for GitHubWriter<T> {
    fn ensure_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError> {
        self.ensure_issue_comment(spec)
    }
    fn mark_ready(&self, spec: &PullRequestReadySpec) -> Result<MutationResult, GitHubError> {
        self.mark_pull_request_ready(spec)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum DispositionCycle {
    Idle,
    AuthorizationBlocked,
    ReadinessPending {
        case_key: String,
        blockers: Vec<String>,
    },
    Recorded {
        effect_id: String,
        effect_type: String,
    },
    Published {
        effect_id: String,
        target_number: u64,
        external_id: u64,
    },
}

#[derive(Debug)]
pub enum DispositionError {
    GitHub(GitHubError),
    Store(StoreError),
    MissingAutomationActor,
    InvalidCase,
    UnexpectedMutationResult,
    Preflight(crate::FinalPreflightError),
    ReadinessBlocked(Vec<String>),
}

impl fmt::Display for DispositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::MissingAutomationActor => {
                formatter.write_str("GitHub disposition requires an automation actor")
            }
            Self::InvalidCase => formatter.write_str("disposition effect case is invalid"),
            Self::UnexpectedMutationResult => {
                formatter.write_str("comment mutation returned a non-comment result")
            }
            Self::Preflight(error) => error.fmt(formatter),
            Self::ReadinessBlocked(blockers) => {
                write!(formatter, "readiness blocked: {}", blockers.join(", "))
            }
        }
    }
}

impl std::error::Error for DispositionError {}

impl From<GitHubError> for DispositionError {
    fn from(error: GitHubError) -> Self {
        Self::GitHub(error)
    }
}

impl From<StoreError> for DispositionError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub fn consume_disposition_once<'a, S: crate::FinalPreflightSource, W: DispositionWriter>(
    (source, writer): (&S, &W),
    scope: impl Into<crate::RepositoryScope<'a>>,
    store: &mut Store,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<DispositionCycle, DispositionError> {
    let scope = scope.into();
    let policy = scope.policy;
    let allowed = if authorization_valid {
        LOCAL_EFFECTS
            .into_iter()
            .chain(COMMENT_EFFECTS)
            .collect::<Vec<_>>()
    } else {
        LOCAL_EFFECTS.to_vec()
    };
    let Some(claimed) = scope.claim(store, owner, now, lease_seconds, allowed.as_slice())? else {
        return Ok(if authorization_valid {
            DispositionCycle::Idle
        } else {
            DispositionCycle::AuthorizationBlocked
        });
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or(DispositionError::InvalidCase)?;
    if case.repository_id != policy.repository.id
        || case.policy_revision != policy.revision
        || case.state_revision != claimed.state_revision
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(DispositionError::InvalidCase);
    }
    if LOCAL_EFFECTS.contains(&claimed.effect_type.as_str()) {
        let evidence = EvidenceInput {
            evidence_id: format!("evidence-effect-{}", claimed.effect_id),
            kind: "LOCAL_EFFECT_COMPLETION".into(),
            source: "pip-control".into(),
            payload: json!({
                "effect_type": claimed.effect_type,
                "state": case.state,
                "state_revision": case.state_revision,
            }),
        };
        store.complete_effect_evidence(&claimed.effect_id, owner, now, &evidence)?;
        return Ok(DispositionCycle::Recorded {
            effect_id: claimed.effect_id,
            effect_type: claimed.effect_type,
        });
    }
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(DispositionError::MissingAutomationActor)?;
    let readiness = if claimed.effect_type == "NOTIFY_SHADOW_READY" {
        match publish_ready(source, writer, store, policy, &case, &claimed.effect_id) {
            Ok(result) => Some(result),
            Err(error) => {
                store.release_effect(&claimed.effect_id, owner)?;
                if let DispositionError::ReadinessBlocked(blockers) = error {
                    return Ok(DispositionCycle::ReadinessPending {
                        case_key: case.case_key,
                        blockers,
                    });
                }
                return Err(error);
            }
        }
    } else {
        None
    };
    let (target_number, body) = disposition_comment(&case, &claimed.effect_type)?;
    let result = match writer.ensure_comment(&CommentSpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        issue_number: target_number,
        effect_id: claimed.effect_id.clone(),
        expected_actor_id: expected_actor,
        body,
    }) {
        Ok(result) => result,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    let (mutation, external_id) = match result {
        MutationResult::Created(id) => ("created", id),
        MutationResult::Existing(id) => ("existing", id),
        MutationResult::Updated(_) | MutationResult::Merged(_) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(DispositionError::UnexpectedMutationResult);
        }
    };
    let evidence = EvidenceInput {
        evidence_id: format!("evidence-effect-{}", claimed.effect_id),
        kind: "GITHUB_DISPOSITION_COMMENT".into(),
        source: format!("github-issue-{target_number}"),
        payload: json!({
            "effect_type": claimed.effect_type,
            "target_number": target_number,
            "comment_id": external_id,
            "mutation": mutation,
            "actor_id": expected_actor,
            "ready_for_review": readiness,
        }),
    };
    let ledger = store.complete_effect_evidence(&claimed.effect_id, owner, now, &evidence)?;
    debug_assert!(matches!(
        ledger,
        ApplyResult::Applied | ApplyResult::Replayed
    ));
    Ok(DispositionCycle::Published {
        effect_id: claimed.effect_id,
        target_number,
        external_id,
    })
}

fn publish_ready<S: crate::FinalPreflightSource, W: DispositionWriter>(
    source: &S,
    writer: &W,
    store: &Store,
    policy: &crate::RepositoryPolicy,
    case: &StoredCase,
    effect_id: &str,
) -> Result<serde_json::Value, DispositionError> {
    if case.state != "SHADOW_READY" {
        return Err(DispositionError::InvalidCase);
    }
    let pr = case.pr_number.ok_or(DispositionError::InvalidCase)?;
    let head = case
        .head_sha
        .as_deref()
        .ok_or(DispositionError::InvalidCase)?;
    let history = store.immutable_history_for_case(&case.case_key)?;
    let final_run = history
        .runs
        .iter()
        .rev()
        .find(|run| run.role == "final-reviewer")
        .ok_or(DispositionError::InvalidCase)?;
    let result: pip_contracts::WorkerResult = serde_json::from_value(final_run.payload.clone())
        .map_err(|_| DispositionError::InvalidCase)?;
    if !crate::draft_pr::payload_matches(&final_run.payload, &final_run.payload_sha256)
        || !matches!(result, pip_contracts::WorkerResult::Final(result)
            if result.outcome == pip_contracts::FinalOutcome::Ready
            && result.common.task_id == final_run.task_id
            && result.plan_version == case.plan_version && result.pr_number == pr
            && result.reviewed_head_sha == head)
    {
        return Err(DispositionError::InvalidCase);
    }
    let issue = source.intake(
        &policy.repository.owner,
        &policy.repository.name,
        case.issue_number,
    )?;
    if !crate::final_preflight::fresh_issue_authorization(policy, case, &issue) {
        return Err(DispositionError::ReadinessBlocked(vec![
            "ISSUE_AUTHORIZATION_REVOKED".into(),
        ]));
    }
    let pull = source.pull_request(
        &policy.repository.owner,
        &policy.repository.name,
        policy.repository.id,
        pr,
    )?;
    let threads = source.review_threads(
        &policy.repository.owner,
        &policy.repository.name,
        policy.repository.id,
        pr,
    )?;
    let blockers =
        crate::final_preflight::final_gate_blockers(store, case, policy, &pull, &threads, None)
            .map_err(DispositionError::Preflight)?;
    if !blockers.is_empty() {
        return Err(DispositionError::ReadinessBlocked(blockers));
    }
    let result = writer.mark_ready(&PullRequestReadySpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        repository_id: policy.repository.id,
        pull_request_number: pr,
        expected_actor_id: policy
            .github
            .automation_actor_id
            .ok_or(DispositionError::MissingAutomationActor)?,
        expected_head_branch: pull.pull_request.head_branch,
        expected_head_sha: head.into(),
        expected_base_branch: policy.repository.default_branch.clone(),
        client_mutation_id: format!("{effect_id}:ready"),
    })?;
    let mutation = match result {
        MutationResult::Existing(id) if id == pr => "existing",
        MutationResult::Updated(id) if id == pr => "updated",
        _ => return Err(DispositionError::UnexpectedMutationResult),
    };
    Ok(json!({"pull_request_number":pr,"head_sha":head,"mutation":mutation}))
}

fn disposition_comment(
    case: &StoredCase,
    effect_type: &str,
) -> Result<(u64, String), DispositionError> {
    let identity = format!(
        "Pip case `{}` at state revision {}",
        case.case_key, case.state_revision
    );
    match effect_type {
        "NOTIFY_SHADOW_READY" => {
            let pr_number = case.pr_number.ok_or(DispositionError::InvalidCase)?;
            let head = case
                .head_sha
                .as_deref()
                .ok_or(DispositionError::InvalidCase)?;
            Ok((
                pr_number,
                format!(
                    "## Pip shadow disposition: ready for human review\n\n{identity} completed its deterministic final review on `{head}`. Autonomous merge is disabled; a human merge decision is required."
                ),
            ))
        }
        "HOLD_FOR_HUMAN" => Ok((
            case.issue_number,
            format!(
                "## Pip is waiting for a human decision\n\n{identity} cannot continue until the issue's authoritative human resolves the recorded scope or product decision."
            ),
        )),
        "ESCALATE" => Ok((
            case.issue_number,
            format!(
                "## Pip workflow escalated\n\n{identity} reached a configured remediation or review bound. Automation is held for human disposition."
            ),
        )),
        _ => Err(DispositionError::InvalidCase),
    }
}
