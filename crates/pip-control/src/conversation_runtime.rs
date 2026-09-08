//! One bounded conversation step using the existing queue and publication adapter.
use std::time::Duration;

use pip_contracts::WorkerRole;
use pip_github::{CommentSpec, DiscussionComment, GitHubError, MutationResult};
use pip_hermes::{CommandRunner, HermesProjector, HermesReader, ProjectionResult, TaskCreateSpec};
use pip_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{DispositionWriter, IntakeSource, RepositoryPolicy};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    schema_version: u32,
    message_key: String,
    reply: String,
    follow_up: String,
    requested_model: String,
    actual_model: String,
    skills_repository_commit: String,
}

pub fn reconcile_conversation_once<
    S: IntakeSource,
    W: DispositionWriter,
    R: CommandRunner + Clone,
>(
    source: &S,
    writer: &W,
    policy: &RepositoryPolicy,
    store: &mut Store,
    runner: R,
    runtime: (&str, &str),
    now: u64,
) -> Result<Value> {
    if !policy.conversations_enabled || policy.intake.paused || !policy.dispatch_enabled {
        return Ok(json!({"result":"disabled"}));
    }
    let (hermes, skills_commit) = runtime;
    if skills_commit.len() != 40
        || !skills_commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid conversation skills commit".into());
    }
    let Some(mut message) = store
        .conversations(policy.repository.id, 1)?
        .into_iter()
        .next()
    else {
        return Ok(json!({"result":"idle"}));
    };
    let key = message.input.key.clone();
    let queue = HermesReader::new(
        runner.clone(),
        hermes,
        Duration::from_secs(20),
        4 * 1024 * 1024,
    )?;
    let tasks = queue.list_tasks(&policy.board)?;
    // Never interrupt a native job or a prepared/running direct attempt. The
    // controller holds new dispatch while inbox feedback awaits this handoff.
    if store.status(now)?.direct_attempts_running > 0
        || tasks.iter().any(|task| {
            Some(&task.id) != message.task_id.as_ref()
            && !matches!(task.status.as_str(), "done" | "cancelled" | "archived")
            // A create with an uncertain response must still be reconciled.
            && !(message.state == "CREATING" && task.body.contains(&key))
        })
    {
        return Ok(json!({"result":"waiting_for_workers","message_key":key}));
    }
    if let Some(id) = &message.task_id
        && tasks
            .iter()
            .any(|t| &t.id == id && t.status != "done" && t.status != "cancelled")
    {
        return Ok(json!({"result":"waiting","message_key":key}));
    }
    let actor = policy
        .github
        .automation_actor_id
        .ok_or("missing automation actor")?;
    let original: DiscussionComment =
        serde_json::from_value(message.input.payload["comment"].clone())?;
    let kind = message.input.payload["kind"]
        .as_str()
        .ok_or("missing comment kind")?;
    let live = source.discussion_comment(
        &policy.repository.owner,
        &policy.repository.name,
        message.input.thread_number,
        kind,
        original.id,
    );
    let unchanged = match live {
        Ok(comment) => same_version(&original, &comment),
        Err(GitHubError::HttpStatus(404)) => false,
        Err(error) => return Err(error.into()),
    };
    if !unchanged
        || message.input.actor_id == actor
        || !policy
            .intake
            .trusted_actor_ids
            .contains(&message.input.actor_id)
    {
        store.finish_conversation(&key, "IGNORED", None)?;
        return Ok(json!({"result":"ignored","message_key":key}));
    }
    let projector = HermesProjector::new(runner, hermes, Duration::from_secs(20), 4 * 1024 * 1024)?;
    if message.state == "PENDING" {
        let context = source.intake(
            &policy.repository.owner,
            &policy.repository.name,
            message.input.thread_number,
        )?;
        if context.repository.id != policy.repository.id
            || context.repository.full_name != policy.repository.full_name()
            || context.issue.number != message.input.thread_number
        {
            return Err("discussion identity changed".into());
        }
        let role = policy
            .roles
            .iter()
            .find(|r| r.role == WorkerRole::Planner && r.is_hermes())
            .ok_or("conversation requires configured native planner model")?;
        let case = message
            .input
            .case_key
            .as_deref()
            .map(|k| store.case(k))
            .transpose()?
            .flatten();
        let plan = if let Some(case) = &case {
            store
                .runs_for_case(&case.case_key)?
                .into_iter()
                .rev()
                .find(|r| r.role == "planner")
                .map(|run| excerpt(&run.payload.to_string(), 16_000))
        } else {
            None
        };
        let spec = TaskCreateSpec {
            board: policy.board.clone(),
            effect_id: key.clone(),
            projection_key: key.clone(),
            title: format!("Respond to discussion #{}", message.input.thread_number),
            body: json!({"schema_version":1,"message_key":key,"repository":policy.repository.full_name(),
                "thread_number":message.input.thread_number,"source_comment":original,
                "title":context.issue_content.title,"issue_body":excerpt(&context.issue_content.body,16_000),
                "recent_comments":context.comments.iter().rev().take(12).map(|c| json!({"id":c.id,"actor_id":c.actor_id,"body":excerpt(&c.body,1500)})).collect::<Vec<_>>(),
                "case":case,"latest_plan_excerpt":plan,"read_only_repository":policy.checkout,
                "provider":role.provider,"model":role.model,"reasoning_effort":role.reasoning_effort,
                "skills_repository_commit":skills_commit,"policy_revision":policy.revision,
                "instructions":"Answer the specific human message using the conversation skill. Do not edit files, run builds, or mutate GitHub. A mention does not authorize a build."}),
            assignee: "conversation".into(),
            workspace: "scratch".into(),
            skills: vec!["conversation".into(), "workflow-contract".into()],
            provider: role.provider.clone(),
            model: role.model.clone(),
            max_runtime: "PT10M".into(),
            max_retries: 1,
            priority: 10,
            parent_task_ids: vec![],
        };
        // Stay below platform argument limits; never split an immutable job.
        if serde_json::to_vec(&spec)?.len() > 100_000 {
            return Err("discussion context exceeds bounded task size".into());
        }
        if store.reserve_conversation(&key, &serde_json::to_value(&spec)?)? {
            let (ProjectionResult::Created(id) | ProjectionResult::Existing(id)) =
                projector.project(&spec, &tasks)?;
            store.bind_conversation_task(&key, &id)?;
            return Ok(json!({"result":"queued","message_key":key,"task_id":id}));
        }
        return Ok(json!({"result":"reserved_elsewhere","message_key":key}));
    }
    let spec: TaskCreateSpec = serde_json::from_value(
        message
            .spec
            .clone()
            .ok_or("missing frozen conversation task")?,
    )?;
    let id = projector
        .reconcile(&spec, &tasks)?
        .ok_or("conversation task creation uncertain; operator recovery required")?;
    if message.state == "CREATING" {
        store.bind_conversation_task(&key, &id)?;
        message = store.conversation(&key)?.ok_or("missing conversation")?;
    }
    if message.task_id.as_deref() != Some(&id) {
        return Err("conversation task binding changed".into());
    }
    if tasks
        .iter()
        .any(|t| t.id == id && t.status != "done" && t.status != "cancelled")
    {
        return Ok(json!({"result":"waiting","message_key":key}));
    }
    if message.state == "QUEUED" {
        let result = queue.show_completed_result(&policy.board, &id)?;
        if projector
            .reconcile(&spec, std::slice::from_ref(&result.task))?
            .as_deref()
            != Some(&id)
        {
            return Err("completed conversation task differs from frozen definition".into());
        }
        if result.profile != "conversation" {
            return Err("conversation profile differs".into());
        }
        let metadata = result.worker_contract_metadata()?;
        validate_answer(&metadata, &key, &spec.body)?;
        store.answer_conversation(&key, &metadata)?;
        message.answer = Some(metadata);
    }
    let metadata = message.answer.as_ref().ok_or("missing saved answer")?;
    let mut answer = validate_answer(metadata, &key, &spec.body)?;
    // Follow-up is deliberately handled before reply publication, with a
    // deterministic ledger event; a publication retry cannot repeat a replan.
    if message.reply_body.is_none() && answer.follow_up == "REPLAN" {
        let disposition = handoff(source, policy, store, &message, &spec, now)?;
        answer.reply.push_str("\n\n");
        answer.reply.push_str(disposition);
    }
    let body = message.reply_body.clone().unwrap_or(answer.reply);
    store.prepare_conversation_reply(&key, &body)?;
    let result = writer.ensure_comment(&CommentSpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        issue_number: message.input.thread_number,
        effect_id: key.clone(),
        expected_actor_id: actor,
        body,
    })?;
    let (MutationResult::Created(reply) | MutationResult::Existing(reply)) = result else {
        return Err("unexpected conversation publication result".into());
    };
    store.finish_conversation(&key, "PUBLISHED", Some(reply))?;
    Ok(json!({"result":"published","message_key":key,"reply_id":reply}))
}

fn handoff<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    message: &pip_store::Conversation,
    spec: &TaskCreateSpec,
    now: u64,
) -> Result<&'static str> {
    use pip_controller::{LedgerController, WorkflowCommand};
    use pip_core::*;
    use std::{
        num::{NonZeroU32, NonZeroU64},
        str::FromStr,
    };
    const REPLANNED: &str = "I've sent this feedback back to planning. Any code changes will still go through the normal build and review checks.";
    const BOUNDED: &str = "I've retained this feedback, but the workflow has reached its revision limit and needs an operator's attention.";
    const DISCUSSION_ONLY: &str = "No build was started. Work needs an active, authorized Pip case; a mention alone doesn't grant that authorization.";
    let Some(key) = &message.input.case_key else {
        return Ok(DISCUSSION_ONLY);
    };
    let case = store.case(key)?.ok_or("conversation case disappeared")?;
    let history = store.immutable_history_for_case(key)?;
    let event_id = format!("event-{}", message.input.key);
    if let Some(event) = history
        .events
        .iter()
        .find(|event| event.event_id == event_id)
    {
        return Ok(if event.payload["bounded"] == true {
            BOUNDED
        } else {
            REPLANNED
        });
    }
    let state = CaseState::from_str(&case.state)?;
    if !matches!(
        state,
        CaseState::Planning
            | CaseState::WaitingHuman
            | CaseState::ReadyToBuild
            | CaseState::WaitingCi
            | CaseState::Reviewing
            | CaseState::Remediating
            | CaseState::FinalReview
            | CaseState::ShadowReady
    ) || policy
        .intake
        .held_issue_numbers
        .contains(&case.issue_number)
        || !crate::verify_active_authorization(
            source,
            crate::RepositoryScope::case(policy, key),
            store,
        )?
        .is_authorized()
    {
        return Ok(DISCUSSION_ONLY);
    }
    if spec.body["case"]["head_sha"] != json!(case.head_sha)
        || spec.body["case"]["plan_version"] != json!(case.plan_version)
    {
        return Err(
            "case head or plan changed during conversation; operator reassessment required".into(),
        );
    }
    let bounded = case.remediation_round >= policy.max_remediation_rounds;
    let case_id = CaseId::new(
        RepositoryId::new(NonZeroU64::new(case.repository_id).ok_or("invalid repository")?),
        IssueNumber::new(NonZeroU64::new(case.issue_number).ok_or("invalid issue")?),
        WorkflowVersion::new(NonZeroU32::new(case.workflow_version).ok_or("invalid workflow")?),
    );
    if case_id.to_string() != *key {
        return Err("case identity differs".into());
    }
    LedgerController::apply(
        store,
        &policy.case_policy(),
        &WorkflowCommand {
            case_id,
            event_id: EventId::from_str(&event_id)?,
            observed_at: ObservedAt::new(now),
            expected_state: state,
            expected_state_revision: StateRevision::new(
                NonZeroU64::new(case.state_revision).ok_or("invalid revision")?,
            ),
            accepted_policy_revision: PolicyRevision::new(
                NonZeroU64::new(case.policy_revision).ok_or("invalid policy")?,
            ),
            remediation_round: case.remediation_round,
            plan_version: NonZeroU32::new(case.plan_version).map(PlanVersion::new),
            pr_number: case
                .pr_number
                .and_then(NonZeroU64::new)
                .map(PullRequestNumber::new),
            head_sha: case.head_sha.as_deref().map(GitSha::from_str).transpose()?,
            event: Event::HumanFeedbackReceived,
            accepted_plan_version: None,
            next_pr_number: None,
            next_head_sha: None,
            event_payload: json!({"message_key":message.input.key,"bounded":bounded}),
            run: None,
            findings: vec![],
            evidence: vec![pip_store::EvidenceInput {
                evidence_id: format!("evidence-{}", message.input.key),
                kind: "HUMAN_DISCUSSION".into(),
                source: format!("github-thread-{}", message.input.thread_number),
                payload: json!({"message":message.input,"assessment":message.answer,"previous_head_sha":case.head_sha}),
            }],
        },
    )?;
    Ok(if bounded { BOUNDED } else { REPLANNED })
}

fn validate_answer(value: &Value, key: &str, binding: &Value) -> Result<Answer> {
    let answer: Answer = serde_json::from_value(value.clone())?;
    if answer.schema_version != 1
        || answer.message_key != key
        || answer.reply.trim().is_empty()
        || answer.reply.len() > 8_000
        || !matches!(answer.follow_up.as_str(), "NONE" | "REPLAN")
        || answer.reply.contains("<!--")
        || answer.requested_model
            != format!(
                "{}/{}",
                binding["provider"].as_str().ok_or("missing provider")?,
                binding["model"].as_str().ok_or("missing model")?
            )
        || answer.actual_model != answer.requested_model
        || Some(answer.skills_repository_commit.as_str())
            != binding["skills_repository_commit"].as_str()
    {
        return Err("invalid conversation result".into());
    }
    Ok(answer)
}

fn excerpt(value: &str, limit: usize) -> String {
    let mut text: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        text.push_str("\n[excerpt truncated]");
    }
    text
}

fn same_version(original: &DiscussionComment, live: &DiscussionComment) -> bool {
    // GitHub may relocate or obsolete a review line when a new commit arrives.
    // That is not an edit/retraction of the human's message.
    live.human
        && live.id == original.id
        && live.actor_id == original.actor_id
        && live.body == original.body
        && live.updated_at == original.updated_at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_review_line_does_not_discard_unchanged_human_feedback() {
        let original: DiscussionComment = serde_json::from_value(json!({"id":1,"actor_id":2,"human":true,
            "body":"Please consider this","updated_at":"2026-09-08","reply_to":null,"context":{"line":12}})).unwrap();
        let mut live = original.clone();
        live.context = json!({"line":null,"original_line":12});
        assert!(same_version(&original, &live));
        live.body = "Edited request".into();
        assert!(!same_version(&original, &live));
    }

    #[test]
    fn answer_requires_exact_model_and_skill_binding() {
        let binding = json!({"provider":"configured-provider","model":"configured-model","skills_repository_commit":"a".repeat(40)});
        let mut answer = json!({"schema_version":1,"message_key":"message-1","reply":"An explanation","follow_up":"NONE",
            "requested_model":"configured-provider/configured-model","actual_model":"configured-provider/configured-model","skills_repository_commit":"a".repeat(40)});
        assert!(validate_answer(&answer, "message-1", &binding).is_ok());
        answer["actual_model"] = json!("different-provider/different-model");
        assert!(validate_answer(&answer, "message-1", &binding).is_err());
    }
}
