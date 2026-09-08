//! Human GitHub conversation intake; never issue/build authorization.
use crate::{
    ActiveIntakeError, IntakeSource, RepositoryPolicy, WebhookEnvelope, WebhookIntakeReport,
};
use pip_store::{ApplyResult, ConversationInput, Store, WebhookDeliveryInput};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) fn comment_event(name: &str) -> bool {
    matches!(
        name,
        "issue_comment" | "pull_request_review_comment" | "pull_request_review"
    )
}

pub(crate) fn ingest<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    envelope: WebhookEnvelope<'_>,
    now: u64,
) -> Result<WebhookIntakeReport, ActiveIntakeError> {
    let value: Value = serde_json::from_slice(envelope.payload)
        .map_err(|_| ActiveIntakeError::InvalidWebhook("malformed comment event"))?;
    let action = value["action"]
        .as_str()
        .ok_or(ActiveIntakeError::InvalidIdentity)?;
    if value["repository"]["id"].as_u64() != Some(policy.repository.id)
        || value["repository"]["full_name"].as_str() != Some(policy.repository.full_name().as_str())
    {
        return Err(ActiveIntakeError::InvalidIdentity);
    }
    let recorded = store.record_webhook_delivery(&WebhookDeliveryInput {
        delivery_id: envelope.delivery_id.into(),
        repository_id: policy.repository.id,
        event_name: envelope.event_name.into(),
        action: action.into(),
        received_at: envelope.received_at,
        payload_sha256: digest(envelope.payload),
    })?;
    let report = |delivery: &str| WebhookIntakeReport {
        report_format: 1,
        observed_at: now,
        repository_id: policy.repository.id,
        repository: policy.repository.full_name(),
        delivery_id: envelope.delivery_id.into(),
        delivery: delivery.into(),
        mutation_count: u64::from(recorded == ApplyResult::Applied),
        candidate: None,
    };
    if !policy.conversations_enabled || !matches!(action, "created" | "edited" | "submitted") {
        return Ok(report("IGNORED"));
    }
    let actor = value["sender"]["id"]
        .as_u64()
        .ok_or(ActiveIntakeError::InvalidIdentity)?;
    let Some(automation) = policy.github.automation_actor_id else {
        return Ok(report("IGNORED"));
    };
    if actor == automation
        || [
            policy.github.reviewer_general_actor_id,
            policy.github.reviewer_secperf_actor_id,
        ]
        .contains(&Some(actor))
        || !policy.intake.trusted_actor_ids.contains(&actor)
    {
        return Ok(report("IGNORED"));
    }
    let comment = if envelope.event_name == "pull_request_review" {
        &value["review"]
    } else {
        &value["comment"]
    };
    let id = comment["id"]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or(ActiveIntakeError::InvalidIdentity)?;
    let thread = value["issue"]["number"]
        .as_u64()
        .or_else(|| value["pull_request"]["number"].as_u64())
        .filter(|n| *n > 0)
        .ok_or(ActiveIntakeError::InvalidIdentity)?;
    let live = match source.discussion_comment(
        &policy.repository.owner,
        &policy.repository.name,
        thread,
        envelope.event_name,
        id,
    ) {
        Ok(live) => live,
        Err(pip_github::GitHubError::HttpStatus(404)) => return Ok(report("IGNORED")),
        Err(error) => return Err(evidence_error(error)),
    };
    if live.actor_id != actor
        || !live.human
        || live.body.trim().is_empty()
        || comment["body"].as_str() != Some(&live.body)
    {
        return Ok(report("IGNORED"));
    }
    let key = format!(
        "conversation-{}-{}-{id}-{}",
        policy.repository.id,
        envelope.event_name,
        digest(format!("{}\n{}", live.updated_at, live.body).as_bytes())
    );
    if store.conversation(&key)?.is_some() {
        return Ok(report("REPLAYED"));
    }
    let case = store
        .status(now)?
        .cases
        .into_iter()
        .filter(|c| {
            c.repository_id == policy.repository.id
                && (c.issue_number == thread || c.pr_number == Some(thread))
        })
        .max_by_key(|c| c.workflow_version);
    let login = source.actor_login(automation).map_err(evidence_error)?;
    let mut relevant = mentions(&live.body, &login)
        || case
            .as_ref()
            .is_some_and(|c| c.pr_number == Some(thread) || c.state == "WAITING_HUMAN");
    if !relevant {
        if let Some(parent) = live.reply_to {
            relevant = source
                .discussion_comment(
                    &policy.repository.owner,
                    &policy.repository.name,
                    thread,
                    envelope.event_name,
                    parent,
                )
                .map_err(evidence_error)?
                .actor_id
                == automation;
        } else if envelope.event_name == "issue_comment" {
            let context = source
                .intake(&policy.repository.owner, &policy.repository.name, thread)
                .map_err(evidence_error)?;
            relevant = context
                .comments
                .iter()
                .filter(|c| c.id < id)
                .max_by_key(|c| c.id)
                .is_some_and(|c| c.actor_id == automation);
        }
    }
    if !relevant {
        return Ok(report("IGNORED"));
    }
    if store.conversations(policy.repository.id, 100)?.len() >= 100 {
        return Err(ActiveIntakeError::Evidence(crate::ShadowError::Evidence(
            "conversation inbox full; retain delivery for retry".into(),
        )));
    }
    store.record_conversation(&ConversationInput {
        key,
        repository_id: policy.repository.id,
        thread_number: thread,
        actor_id: actor,
        case_key: case.map(|c| c.case_key),
        received_at: envelope.received_at,
        payload: json!({"kind":envelope.event_name,"comment":live}),
    })?;
    let mut queued = report("QUEUED");
    queued.mutation_count += 1;
    Ok(queued)
}

fn evidence_error(error: pip_github::GitHubError) -> ActiveIntakeError {
    ActiveIntakeError::Evidence(crate::ShadowError::Evidence(error.to_string()))
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn mentions(body: &str, login: &str) -> bool {
    let mut fence = false;
    body.lines().any(|line| {
        let line = line.trim_start();
        if line.starts_with("```") || line.starts_with("~~~") {
            fence = !fence;
            return false;
        }
        if fence || line.starts_with('>') {
            return false;
        }
        line.split('`').step_by(2).any(|plain| {
            plain
                .split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '@' | '-' | '_'))
                .any(|word| word.eq_ignore_ascii_case(&format!("@{login}")))
        })
    })
}
