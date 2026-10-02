//! Applies `@pip resume|replan|abandon` commands to parked cases.
//!
//! A parked case waits for a trusted human. These commands are the ordinary,
//! online way to continue: no root, no stopped services, no ledger surgery.
use std::fmt;
use std::str::FromStr;

use pip_controller::{ControllerError, LedgerController};
use pip_core::{CaseState, Event, EventId};
use pip_github::{CommentSpec, GitHubError};
use pip_store::{
    ControlCommand, ControlCommandStatus, EvidenceInput, PARKED_STATES, Store, StoreError,
};
use serde::Serialize;
use serde_json::json;

use crate::{DispositionWriter, IntakeSource, RepositoryPolicy};

/// Remediation rounds granted when a human resumes a case that used them up.
pub const RESUME_GRANTED_ROUNDS: u32 = 3;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ResumeCycle {
    Idle,
    Applied {
        case_key: String,
        comment_id: u64,
        command: String,
        state: String,
    },
    Ignored {
        comment_id: u64,
        reason: String,
    },
    Waiting {
        comment_id: u64,
        reason: String,
    },
}

#[derive(Debug)]
pub enum ResumeError {
    Store(StoreError),
    Controller(ControllerError),
    Intake(crate::ActiveIntakeError),
    Invalid(String),
}

impl fmt::Display for ResumeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::Intake(error) => error.fmt(formatter),
            Self::Invalid(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for ResumeError {}

impl From<StoreError> for ResumeError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ControllerError> for ResumeError {
    fn from(error: ControllerError) -> Self {
        Self::Controller(error)
    }
}

impl From<crate::ActiveIntakeError> for ResumeError {
    fn from(error: crate::ActiveIntakeError) -> Self {
        Self::Intake(error)
    }
}

/// Inputs the controller cycle has already established for this case.
pub struct ResumeContext<'a> {
    pub policy: &'a RepositoryPolicy,
    pub case_key: &'a str,
    pub now: u64,
    /// Issue authorization holds, apart from a policy change a resume fixes.
    pub authorized: bool,
}

pub fn apply_control_commands_once<S: IntakeSource, W: DispositionWriter>(
    source: &S,
    writer: &W,
    store: &mut Store,
    context: &ResumeContext<'_>,
) -> Result<ResumeCycle, ResumeError> {
    let policy = context.policy;
    let Some(command) = store
        .pending_control_commands(policy.repository.id, Some(context.case_key))?
        .into_iter()
        .next()
    else {
        return Ok(ResumeCycle::Idle);
    };
    let case = store
        .case(context.case_key)?
        .ok_or_else(|| ResumeError::Invalid("control command case is missing".into()))?;
    let ignore = |store: &mut Store, reason: &str| -> Result<ResumeCycle, ResumeError> {
        store.resolve_control_command(
            command.comment_id,
            ControlCommandStatus::Ignored,
            reason,
            context.now,
        )?;
        Ok(ResumeCycle::Ignored {
            comment_id: command.comment_id,
            reason: reason.into(),
        })
    };
    if !PARKED_STATES.contains(&case.state.as_str()) {
        return ignore(store, "the case is not paused");
    }
    let park = store
        .parking_event(&case.case_key)?
        .ok_or_else(|| ResumeError::Invalid("parked case has no parking event".into()))?;
    if command.received_at < park.observed_at {
        return ignore(store, "the command predates the current pause");
    }
    let waiting = |reason: &str| ResumeCycle::Waiting {
        comment_id: command.comment_id,
        reason: reason.into(),
    };
    if command.command == "ABANDON" {
        apply(
            store,
            &case,
            &command,
            Event::HumanAbandoned,
            None,
            0,
            context,
        )?;
        acknowledge(
            source,
            writer,
            policy,
            &command,
            case.issue_number,
            "Stopped working on this issue. Re-add the authorization label to start over.",
        );
        return applied(store, &case.case_key, &command, context.now);
    }
    if !context.authorized {
        return Ok(waiting("issue authorization could not be confirmed"));
    }
    // A running builder could still be writing the worktree. Hermes jobs never
    // write it, and their late results are fenced by the state revision.
    if store.live_direct_attempts_for_case(&case.case_key, context.now)? > 0 {
        return Ok(waiting("a previous worker is still running"));
    }
    if !capacity_available(store, policy, &case.case_key, context.now)? {
        acknowledge(
            source,
            writer,
            policy,
            &command,
            case.issue_number,
            "Queued: Pip is at its active-issue limit and will resume this case when a slot frees up.",
        );
        return Ok(waiting("the active-issue limit is reached"));
    }
    let target = if command.command == "REPLAN" {
        CaseState::Planning
    } else {
        crate::park::resume_target(&park)
    };
    let granted = if crate::park::rounds_exhausted(&park, &case.state) {
        RESUME_GRANTED_ROUNDS
    } else {
        0
    };
    apply(
        store,
        &case,
        &command,
        Event::HumanResumed,
        Some(target),
        granted,
        context,
    )?;
    let rounds = if granted > 0 {
        format!(" with {granted} more remediation rounds")
    } else {
        String::new()
    };
    acknowledge(
        source,
        writer,
        policy,
        &command,
        case.issue_number,
        &format!(
            "Resuming at {}{rounds}.",
            crate::park::stage(&target.to_string())
        ),
    );
    applied(store, &case.case_key, &command, context.now)
}

/// How often a parked case's issue comments are read as a fallback for
/// webhook deliveries that never arrived.
pub const COMMAND_POLL_SECONDS: u64 = 300;

/// Records `@pip` commands on a parked case's issue that the webhook missed.
/// Runs roughly every `COMMAND_POLL_SECONDS`; idempotent per comment.
pub fn poll_control_commands<S: IntakeSource>(
    source: &S,
    store: &mut Store,
    policy: &RepositoryPolicy,
    case_key: &str,
) -> Result<u64, ResumeError> {
    let Some(case) = store.case(case_key)? else {
        return Ok(0);
    };
    let Some(park) = store.parking_event(case_key)? else {
        return Ok(0);
    };
    let Some(automation) = policy.github.automation_actor_id else {
        return Ok(0);
    };
    let evidence = |error: GitHubError| ResumeError::Invalid(error.to_string());
    let login = source.actor_login(automation).map_err(evidence)?;
    let snapshot = source
        .intake(
            &policy.repository.owner,
            &policy.repository.name,
            case.issue_number,
        )
        .map_err(evidence)?;
    let mut recorded = 0;
    for comment in snapshot.comments {
        let Some(created) = unix_seconds(&comment.created_at) else {
            continue;
        };
        if created < park.observed_at
            || comment.created_at != comment.updated_at
            || comment.actor_id == automation
            || !policy.intake.trusted_actor_ids.contains(&comment.actor_id)
        {
            continue;
        }
        let Some((command, guidance)) =
            crate::conversations::parse_control_command(&comment.body, &login)
        else {
            continue;
        };
        let result = store.record_control_command(&pip_store::ControlCommandInput {
            comment_id: comment.id,
            repository_id: policy.repository.id,
            thread_number: case.issue_number,
            case_key: Some(case.case_key.clone()),
            actor_id: comment.actor_id,
            command: command.into(),
            guidance,
            received_at: created,
        })?;
        recorded += u64::from(result == pip_store::ApplyResult::Applied);
    }
    Ok(recorded)
}

/// Parses GitHub's `YYYY-MM-DDTHH:MM:SSZ` timestamps.
pub(crate) fn unix_seconds(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    let field = |range: std::ops::Range<usize>| value.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

fn apply(
    store: &mut Store,
    case: &pip_store::StoredCase,
    command: &ControlCommand,
    event: Event,
    target: Option<CaseState>,
    granted: u32,
    context: &ResumeContext<'_>,
) -> Result<(), ResumeError> {
    let policy = context.policy;
    // A human explicitly accepted today's settings for this case.
    crate::intake::record_case_policy(store, policy, context.now)?;
    let accepted = crate::load_repository_policy(
        &serde_json::to_vec(&store.accepted_policy(case.repository_id, case.policy_revision)?)
            .map_err(|error| ResumeError::Invalid(error.to_string()))?,
    )
    .map_err(|error| ResumeError::Invalid(error.to_string()))?;
    let event_id = EventId::from_str(&format!(
        "event-control-{}-{}",
        command.comment_id, case.state_revision
    ))
    .map_err(|error| ResumeError::Invalid(error.to_string()))?;
    let mut workflow = crate::builder_retry::recovery_command(
        case,
        case.state_revision,
        event_id,
        event,
        json!({
            "comment_id": command.comment_id,
            "actor_id": command.actor_id,
            "command": command.command,
            "resume_target": target.map(|state| state.to_string()),
            "granted_rounds": granted,
            "policy_revision": policy.revision,
            "guidance_sha256": crate::conversations::digest(command.guidance.as_bytes()),
        }),
        context.now,
    )
    .map_err(ResumeError::Invalid)?;
    if !command.guidance.trim().is_empty() {
        // Retained as human discussion so every later worker sees it.
        workflow.evidence.push(EvidenceInput {
            evidence_id: format!("evidence-control-{}", command.comment_id),
            kind: "HUMAN_DISCUSSION".into(),
            source: format!("github-comment-{}", command.comment_id),
            payload: json!({
                "kind": "control_command",
                "command": command.command,
                "actor_id": command.actor_id,
                "body": command.guidance,
            }),
        });
    }
    LedgerController::apply(store, &accepted.case_policy(), &workflow)?;
    Ok(())
}

fn applied(
    store: &mut Store,
    case_key: &str,
    command: &ControlCommand,
    now: u64,
) -> Result<ResumeCycle, ResumeError> {
    let case = store
        .case(case_key)?
        .ok_or_else(|| ResumeError::Invalid("case disappeared".into()))?;
    store.resolve_control_command(
        command.comment_id,
        ControlCommandStatus::Applied,
        &format!("{} -> {}", command.command, case.state),
        now,
    )?;
    Ok(ResumeCycle::Applied {
        case_key: case.case_key,
        comment_id: command.comment_id,
        command: command.command.clone(),
        state: case.state,
    })
}

fn capacity_available(
    store: &Store,
    policy: &RepositoryPolicy,
    case_key: &str,
    now: u64,
) -> Result<bool, ResumeError> {
    let cases = store.status(now)?.cases;
    let active = cases
        .iter()
        .filter(|case| case.case_key != case_key && crate::intake::active_state(&case.state));
    let repository = active
        .clone()
        .filter(|case| case.repository_id == policy.repository.id)
        .count();
    let global = active.count();
    Ok(repository < policy.intake.repository_active_limit as usize
        && global < policy.intake.global_active_limit as usize)
}

/// Best effort: the command is already applied or recorded; a failed reply
/// does not undo it and is retried by the idempotent marker on the next try.
fn acknowledge<S: IntakeSource, W: DispositionWriter>(
    source: &S,
    writer: &W,
    policy: &RepositoryPolicy,
    command: &ControlCommand,
    issue_number: u64,
    message: &str,
) {
    let Some(actor) = policy.github.automation_actor_id else {
        return;
    };
    let requester = source
        .actor_login(command.actor_id)
        .map(|login| format!("@{login} "))
        .unwrap_or_default();
    let _: Result<_, GitHubError> = writer.ensure_comment(&CommentSpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        issue_number,
        effect_id: format!(
            "control-ack-{}-{}",
            command.comment_id,
            digest_prefix(message)
        ),
        expected_actor_id: actor,
        body: format!("{requester}{message}"),
    });
}

fn digest_prefix(message: &str) -> String {
    crate::conversations::digest(message.as_bytes())[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::unix_seconds;

    #[test]
    fn github_timestamps_parse_to_unix_seconds() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("2026-09-07T00:00:00Z"), Some(1_788_739_200));
        assert_eq!(unix_seconds("2024-02-29T12:34:56Z"), Some(1_709_210_096));
        for invalid in [
            "2026-09-07",
            "2026-13-01T00:00:00Z",
            "2026-09-07T24:00:00Z",
            "",
        ] {
            assert_eq!(unix_seconds(invalid), None, "{invalid}");
        }
    }
}
