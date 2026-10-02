use std::cell::RefCell;

use pip_control::{
    DispositionWriter, IntakeSource, RepositoryPolicy, ResumeContext, ResumeCycle,
    apply_control_commands_once, load_repository_policy,
};
use pip_github::{
    CommentSpec, GitHubError, IntakeSnapshot, IssueSnapshot, MutationResult, PullRequestReadySpec,
};
use pip_store::{
    ControlCommandInput, EffectInput, EventInput, NewCase, PolicyInput, Store, TransitionInput,
};
use serde_json::json;

const CASE: &str = "repo:1055628515#42@2";

struct Source;

impl IntakeSource for Source {
    fn actor_login(&self, id: u64) -> Result<String, GitHubError> {
        Ok(if id == 202_880 { "jg" } else { "agent-p1p" }.into())
    }
    fn discover(&self, _: &str, _: &str, _: &str) -> Result<Vec<IssueSnapshot>, GitHubError> {
        unreachable!("resume never discovers issues")
    }
    fn intake(&self, _: &str, _: &str, _: u64) -> Result<IntakeSnapshot, GitHubError> {
        unreachable!("resume uses the cycle's authorization")
    }
}

#[derive(Default)]
struct Writer(RefCell<Vec<CommentSpec>>);

impl DispositionWriter for Writer {
    fn ensure_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError> {
        self.0.borrow_mut().push(spec.clone());
        Ok(MutationResult::Created(900))
    }
    fn mark_ready(&self, _: &PullRequestReadySpec) -> Result<MutationResult, GitHubError> {
        unreachable!()
    }
}

fn policy(revision: u64) -> RepositoryPolicy {
    let mut policy = load_repository_policy(include_bytes!("fixtures/mdk-rev11.json")).unwrap();
    policy.revision = revision;
    policy
}

fn record(store: &mut Store, policy: &RepositoryPolicy) {
    let mut value = serde_json::to_value(policy).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("conversations_enabled");
    value.as_object_mut().unwrap().remove("conversation_model");
    store
        .record_policy(&PolicyInput {
            repository_id: policy.repository.id,
            revision: policy.revision,
            accepted_at: 1,
            payload: value,
        })
        .unwrap();
}

/// A case escalated after spending its last remediation round on red CI.
fn parked_case(store: &mut Store, policy: &RepositoryPolicy, key: &str, issue: u64) {
    store
        .create_case(&NewCase {
            case_key: key.into(),
            repository_id: policy.repository.id,
            issue_number: issue,
            workflow_version: 2,
            policy_revision: policy.revision,
            initial_state: "WAITING_CI".into(),
            observed_at: 100,
            event: EventInput {
                event_id: format!("intake-{issue}"),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: key.into(),
                expected_revision: 1,
                next_state: "ESCALATED".into(),
                remediation_round: policy.max_remediation_rounds,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 200,
                event: EventInput {
                    event_id: format!("ci-failed-{issue}"),
                    event_type: "CI_FAILED".into(),
                    payload: json!({}),
                },
                run: None,
                evidence: vec![],
                findings: vec![],
                effects: vec![EffectInput {
                    effect_id: format!("escalate-{issue}"),
                    effect_type: "ESCALATE".into(),
                    payload: json!({}),
                }],
            },
            None,
        )
        .unwrap();
}

fn command(store: &mut Store, verb: &str, received_at: u64) {
    store
        .record_control_command(&ControlCommandInput {
            comment_id: 555,
            repository_id: 1_055_628_515,
            thread_number: 42,
            case_key: Some(CASE.into()),
            actor_id: 202_880,
            command: verb.into(),
            guidance: "The flaky integration job is fixed on master; rebase onto it.".into(),
            received_at,
        })
        .unwrap();
}

fn run(store: &mut Store, policy: &RepositoryPolicy, writer: &Writer, now: u64) -> ResumeCycle {
    apply_control_commands_once(
        &Source,
        writer,
        store,
        &ResumeContext {
            policy,
            case_key: CASE,
            now,
            authorized: true,
        },
    )
    .unwrap()
}

#[test]
fn resuming_an_exhausted_case_grants_rounds_rebinds_policy_and_keeps_guidance() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    let old = policy(11);
    record(&mut store, &old);
    parked_case(&mut store, &old, CASE, 42);
    command(&mut store, "RESUME", 300);
    let writer = Writer::default();
    // The live policy changed while the case was parked; resume accepts it.
    let live = policy(12);

    assert_eq!(
        run(&mut store, &live, &writer, 310),
        ResumeCycle::Applied {
            case_key: CASE.into(),
            comment_id: 555,
            command: "RESUME".into(),
            state: "REMEDIATING".into(),
        }
    );
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(case.policy_revision, 12);
    assert_eq!(case.remediation_round, old.max_remediation_rounds);
    assert_eq!(store.granted_remediation_rounds(CASE).unwrap(), 3);
    let history = store.immutable_history_for_case(CASE).unwrap();
    let guidance = history
        .evidence
        .iter()
        .find(|evidence| evidence.kind == "HUMAN_DISCUSSION")
        .unwrap();
    assert!(
        guidance.payload["body"]
            .as_str()
            .unwrap()
            .contains("rebase onto it")
    );
    let comments = writer.0.borrow();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].issue_number, 42);
    assert!(
        comments[0].body.contains(
            "@jg Resuming at fixing review or CI feedback with 3 more remediation rounds."
        ),
        "{}",
        comments[0].body
    );
    drop(comments);
    // The command is consumed exactly once.
    assert_eq!(run(&mut store, &live, &writer, 320), ResumeCycle::Idle);
}

#[test]
fn replan_and_abandon_take_effect_from_any_pause() {
    for (verb, state) in [("REPLAN", "PLANNING"), ("ABANDON", "ABANDONED")] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        let policy = policy(11);
        record(&mut store, &policy);
        parked_case(&mut store, &policy, CASE, 42);
        command(&mut store, verb, 300);
        let writer = Writer::default();
        assert!(matches!(
            run(&mut store, &policy, &writer, 310),
            ResumeCycle::Applied { state: ref observed, .. } if observed == state
        ));
        assert_eq!(store.case(CASE).unwrap().unwrap().state, state);
    }
}

#[test]
fn a_command_waits_for_a_running_builder_and_for_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    let mut policy = policy(11);
    policy.intake.repository_active_limit = 1;
    record(&mut store, &policy);
    parked_case(&mut store, &policy, CASE, 42);
    command(&mut store, "RESUME", 300);
    let writer = Writer::default();

    // Another issue is actively being worked on and fills the only slot.
    store
        .create_case(&NewCase {
            case_key: "repo:1055628515#43@2".into(),
            repository_id: policy.repository.id,
            issue_number: 43,
            workflow_version: 2,
            policy_revision: policy.revision,
            initial_state: "BUILDING".into(),
            observed_at: 100,
            event: EventInput {
                event_id: "intake-43".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    assert!(matches!(
        run(&mut store, &policy, &writer, 310),
        ResumeCycle::Waiting { ref reason, .. } if reason.contains("limit")
    ));
    assert!(writer.0.borrow()[0].body.contains("Queued"));
    assert_eq!(store.case(CASE).unwrap().unwrap().state, "ESCALATED");

    // Parked cases themselves never hold a slot. (A policy change is a new revision.)
    policy.intake.repository_active_limit = 2;
    policy.revision = 12;
    assert!(matches!(
        run(&mut store, &policy, &writer, 320),
        ResumeCycle::Applied { .. }
    ));
}

#[test]
fn stale_or_misdirected_commands_are_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    let policy = policy(11);
    record(&mut store, &policy);
    parked_case(&mut store, &policy, CASE, 42);
    // Received before the current pause began.
    command(&mut store, "RESUME", 150);
    let writer = Writer::default();
    assert!(matches!(
        run(&mut store, &policy, &writer, 310),
        ResumeCycle::Ignored { ref reason, .. } if reason.contains("predates")
    ));
    assert_eq!(store.case(CASE).unwrap().unwrap().state, "ESCALATED");
    assert!(writer.0.borrow().is_empty());
}
