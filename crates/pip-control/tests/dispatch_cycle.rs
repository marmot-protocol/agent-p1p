use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use pip_control::{
    DispatchCycleContext, DispatchCycleError, DispatchCycleResult, WorkspaceError,
    WorkspacePreparer, dispatch_once_with, dispatch_once_with_workspace, load_repository_policy,
};
use pip_hermes::{CommandOutput, CommandRunner, CommandSpec, HermesError, TaskSnapshot};
use pip_store::{
    ClaimedEffect, EffectInput, EventInput, NewCase, PolicyInput, Store, StoredCase,
    TransitionInput,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<CommandOutput>>>,
    commands: Rc<RefCell<Vec<CommandSpec>>>,
}

impl FakeRunner {
    fn output(&self, status: i32, value: impl Into<Vec<u8>>) {
        self.outputs.borrow_mut().push_back(CommandOutput {
            status,
            stdout: value.into(),
            stderr: Vec::new(),
            timed_out: false,
        });
    }

    fn json(&self, value: &impl serde::Serialize) {
        self.output(0, serde_json::to_vec(value).unwrap());
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
        self.commands.borrow_mut().push(spec.clone());
        Ok(self.outputs.borrow_mut().pop_front().unwrap())
    }
}

#[derive(Clone, Default)]
struct FakeWorkspace {
    calls: Rc<RefCell<Vec<String>>>,
    fail: bool,
}

impl WorkspacePreparer for FakeWorkspace {
    fn prepare(
        &self,
        _policy: &pip_control::RepositoryPolicy,
        claimed: &ClaimedEffect,
        case: &StoredCase,
        _store: &Store,
    ) -> Result<(), WorkspaceError> {
        self.calls
            .borrow_mut()
            .push(format!("{}:{}", claimed.effect_id, case.state_revision));
        if self.fail {
            Err(WorkspaceError::InvalidCase)
        } else {
            Ok(())
        }
    }
}

#[test]
fn production_dispatch_boundary_prepares_workspace_before_any_hermes_command() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let runner = FakeRunner::default();
    let workspace = FakeWorkspace {
        fail: true,
        ..FakeWorkspace::default()
    };

    assert!(matches!(
        dispatch_once_with_workspace(
            &mut store,
            &active_policy(),
            runner.clone(),
            &workspace,
            context("controller-1", 100),
        ),
        Err(DispatchCycleError::Workspace(WorkspaceError::InvalidCase))
    ));
    assert_eq!(
        workspace.calls.borrow().as_slice(),
        ["effect-intake-planner:1"]
    );
    assert!(runner.commands.borrow().is_empty());
}

#[test]
fn planner_dispatch_projects_gate_and_worker_then_atomically_acks_outbox() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let policy = active_policy();
    let runner = creation_runner();

    let report = dispatch_once_with(
        &mut store,
        &policy,
        runner.clone(),
        context("controller-1", 100),
    )
    .unwrap();
    assert_eq!(
        report,
        DispatchCycleResult::Projected {
            effect_id: "effect-intake-planner".into(),
            projection_count: 2,
            direct_job_count: 0,
            released_gate_count: 1,
            ledger_result: "applied".into(),
        }
    );
    let status = store.status(101).unwrap();
    assert_eq!(status.task_projections, 2);
    assert_eq!(status.outbox_delivered, 1);
    assert!(
        runner
            .commands
            .borrow()
            .iter()
            .any(|command| { command.args.iter().any(|argument| argument == "complete") })
    );
    let worker_body = projected_worker_body(&runner);
    assert_eq!(
        worker_body["sensitive_scope_categories"],
        json!([
            "CRYPTOGRAPHY",
            "MLS_CGKA",
            "KEY_HANDLING",
            "TRUST_ANCHOR",
            "MEMBERSHIP_AUTHORIZATION",
            "ADMIN_AUTHORIZATION",
            "PUSH_PAYLOAD_CONTEXT"
        ])
    );
    let bundle = &worker_body["immutable_evidence_bundle"];
    assert_eq!(bundle["schema_version"], 1);
    assert_eq!(bundle["case_key"], "repo:1055628515#1240@3");
    assert_eq!(bundle["bound_state_revision"], 1);
    assert_eq!(
        bundle["records"]["events"][0]["event_id"],
        "event-intake-5050325280"
    );
    assert_eq!(
        bundle["records"]["events"][0]["event_type"],
        "ISSUE_AUTHORIZED"
    );
    assert_eq!(
        bundle["records"]["events"][0]["payload"],
        json!({"label": "pip-ok"})
    );
    let expected_digest = bundle["sha256"].as_str().unwrap();
    let mut unsigned = bundle.as_object().unwrap().clone();
    unsigned.remove("sha256");
    assert_eq!(
        expected_digest,
        hex_digest(&Sha256::digest(
            serde_json::to_vec(&Value::Object(unsigned)).unwrap()
        ))
    );

    let idle_runner = FakeRunner::default();
    assert_eq!(
        dispatch_once_with(
            &mut store,
            &policy,
            idle_runner.clone(),
            context("controller-2", 102),
        )
        .unwrap(),
        DispatchCycleResult::Idle
    );
    assert!(idle_runner.commands.borrow().is_empty());
}

#[test]
fn direct_builder_is_durably_queued_without_any_hermes_command() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:1055628515#1240@3".into(),
                expected_revision: 1,
                next_state: "BUILDING".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 91,
                event: EventInput {
                    event_id: "event-plan-published".into(),
                    event_type: "PLAN_PUBLISHED".into(),
                    payload: json!({"plan_version": 1}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-dispatch-builder".into(),
                    effect_type: "DISPATCH_BUILDER".into(),
                    payload: json!({"plan_version": 1}),
                }],
            },
            None,
        )
        .unwrap();
    let runner = FakeRunner::default();
    let mut direct_context = context("controller-1", 100);
    direct_context.hermes_program = "";

    assert_eq!(
        dispatch_once_with(&mut store, &active_policy(), runner.clone(), direct_context,).unwrap(),
        DispatchCycleResult::Projected {
            effect_id: "effect-dispatch-builder".into(),
            projection_count: 0,
            direct_job_count: 1,
            released_gate_count: 0,
            ledger_result: "applied".into(),
        }
    );
    assert!(runner.commands.borrow().is_empty());

    let job = store
        .claim_effect_matching("direct-worker-1", 101, 30, &["RUN_DIRECT_WORKER"])
        .unwrap()
        .expect("durable direct worker job");
    assert_eq!(job.case_key, "repo:1055628515#1240@3");
    assert_eq!(job.state_revision, 2);
    assert_eq!(
        job.payload["task_id"],
        "repo:1055628515#1240@3:builder:round:1:revision:2:worker"
    );
    assert_eq!(job.payload["role"], "builder");
    assert_eq!(job.payload["provider"], "cursor");
    assert_eq!(job.payload["model"], "cursor-grok-4.6-high-fast");
    assert_eq!(
        job.payload["workspace"],
        "/var/lib/pip/worktrees/mdk/repo-1055628515-issue-1240-workflow-3"
    );
    assert_eq!(job.payload["body"]["state_revision"], 2);
    assert_eq!(job.payload["body"]["execution"], "direct");
}

#[test]
fn independent_review_dispatch_splits_hermes_and_direct_work_without_model_substitution() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:1055628515#1240@3".into(),
                expected_revision: 1,
                next_state: "REVIEWING".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("c".repeat(40)),
                observed_at: 91,
                event: EventInput {
                    event_id: "event-ci-green".into(),
                    event_type: "CI_GREEN".into(),
                    payload: json!({"head_sha": "c".repeat(40)}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-dispatch-reviewers".into(),
                    effect_type: "DISPATCH_REVIEWERS".into(),
                    payload: json!({"pr_number": 77, "head_sha": "c".repeat(40)}),
                }],
            },
            None,
        )
        .unwrap();
    let runner = review_creation_runner();

    assert_eq!(
        dispatch_once_with(
            &mut store,
            &active_policy(),
            runner.clone(),
            context("controller-1", 100),
        )
        .unwrap(),
        DispatchCycleResult::Projected {
            effect_id: "effect-dispatch-reviewers".into(),
            projection_count: 2,
            direct_job_count: 2,
            released_gate_count: 1,
            ledger_result: "applied".into(),
        }
    );
    assert!(runner.commands.borrow().iter().all(|command| {
        !command
            .args
            .windows(2)
            .any(|pair| pair[0] == "--provider" && pair[1] == "cursor")
    }));
    let job = store
        .claim_effect_matching("direct-worker", 101, 30, &["RUN_DIRECT_WORKER"])
        .unwrap()
        .unwrap();
    assert_eq!(job.payload["role"], "reviewer-secperf");
    assert_eq!(job.payload["body"]["reviewer_id"], "secperf-kimi");
    assert_eq!(job.payload["model"], "kimi-k3-max");
    assert_eq!(job.payload["body"]["expected_head_sha"], "c".repeat(40));
    let shadow = store
        .claim_effect_matching("observer", 101, 30, &["RUN_DIRECT_OBSERVER"])
        .unwrap()
        .unwrap();
    assert_eq!(shadow.payload["body"]["reviewer_id"], "secperf-opus");
    assert_eq!(shadow.payload["model"], "claude-opus-5-thinking-high");
}

fn projected_worker_body(runner: &FakeRunner) -> Value {
    runner
        .commands
        .borrow()
        .iter()
        .find_map(|command| {
            let assignee = command
                .args
                .windows(2)
                .find(|pair| pair[0] == "--assignee")
                .map(|pair| pair[1].as_str());
            if assignee != Some("planner") {
                return None;
            }
            let body = command
                .args
                .windows(2)
                .find(|pair| pair[0] == "--body")?
                .get(1)?;
            serde_json::from_str(body).ok()
        })
        .expect("planner projection command")
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn crash_after_gate_release_reconciles_existing_advanced_tasks_without_duplicates() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let policy = active_policy();
    let first = creation_runner();
    first.outputs.borrow_mut().back_mut().unwrap().status = 2;
    assert!(dispatch_once_with(&mut store, &policy, first, context("controller-1", 100),).is_err());
    assert_eq!(store.task_projection_count().unwrap(), 0);

    let gate = gate("done");
    let worker = worker("ready");
    let recovered = FakeRunner::default();
    recovered.json(&vec![gate.clone(), worker.clone()]);
    recovered.json(&gate);
    recovered.json(&worker);
    let report = dispatch_once_with(
        &mut store,
        &policy,
        recovered.clone(),
        context("controller-2", 131),
    )
    .unwrap();
    assert!(matches!(report, DispatchCycleResult::Projected { .. }));
    assert_eq!(store.task_projection_count().unwrap(), 2);
    assert!(recovered.commands.borrow().iter().all(|command| {
        !command.args.iter().any(|argument| argument == "create")
            && !command.args.iter().any(|argument| argument == "complete")
    }));
}

#[test]
fn invalid_fresh_authorization_never_claims_an_outbox_effect_or_calls_hermes() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let policy = active_policy();
    let runner = FakeRunner::default();
    let mut dispatch = context("controller-1", 100);
    dispatch.authorization_valid = false;

    assert_eq!(
        dispatch_once_with(&mut store, &policy, runner.clone(), dispatch).unwrap(),
        DispatchCycleResult::AuthorizationBlocked
    );
    assert!(runner.commands.borrow().is_empty());
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
}

fn context(owner: &str, now: u64) -> DispatchCycleContext<'_> {
    DispatchCycleContext {
        skills_repository_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        hermes_program: "hermes",
        owner,
        now,
        lease_seconds: 30,
        authorization_valid: true,
    }
}

fn active_policy() -> pip_control::RepositoryPolicy {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["intake"]["enabled"] = json!(true);
    value["intake"]["paused"] = json!(false);
    value["dispatch_enabled"] = json!(true);
    value["github"]["automation_actor_id"] = json!(202880);
    value["github"]["reviewer_general_actor_id"] = json!(202881);
    value["github"]["reviewer_secperf_actor_id"] = json!(202882);
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn seeded_store(root: &std::path::Path) -> Store {
    let mut store = Store::open(root.join("ledger.db")).unwrap();
    let policy = active_policy();
    store
        .record_policy(&PolicyInput {
            repository_id: 1_055_628_515,
            revision: policy.revision,
            accepted_at: 90,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:1055628515#1240@3".into(),
            repository_id: 1_055_628_515,
            issue_number: 1240,
            workflow_version: 3,
            policy_revision: policy.revision,
            initial_state: "PLANNING".into(),
            observed_at: 90,
            event: EventInput {
                event_id: "event-intake-5050325280".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label": "pip-ok"}),
            },
            effects: vec![EffectInput {
                effect_id: "effect-intake-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key": "repo:1055628515#1240@3"}),
            }],
        })
        .unwrap();
    store
}

fn creation_runner() -> FakeRunner {
    let runner = FakeRunner::default();
    let gate = gate("blocked");
    let worker = worker("blocked");
    runner.json(&Vec::<TaskSnapshot>::new());
    runner.json(&gate);
    runner.json(&gate);
    runner.json(&worker);
    runner.json(&worker);
    runner.output(0, Vec::new());
    runner
}

fn review_creation_runner() -> FakeRunner {
    let runner = FakeRunner::default();
    let gate = review_gate("blocked");
    let worker = review_worker("blocked");
    runner.json(&Vec::<TaskSnapshot>::new());
    runner.json(&gate);
    runner.json(&gate);
    runner.json(&worker);
    runner.json(&worker);
    runner.output(0, Vec::new());
    runner
}

fn gate(status: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: "gate-1".into(),
        title: "Activate planner for repo:1055628515#1240@3".into(),
        status: status.into(),
        assignee: None,
        created_by: Some("pip-controller".into()),
        body: json!({
            "case_key": "repo:1055628515#1240@3",
            "state_revision": 1,
            "activation_gate": "planner",
            "projection_key": "repo:1055628515#1240@3:planner:round:1:revision:1:gate"
        })
        .to_string(),
    }
}

fn worker(status: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: "worker-1".into(),
        title: "Run planner for repo:1055628515#1240@3".into(),
        status: status.into(),
        assignee: Some("planner".into()),
        created_by: Some("pip-controller".into()),
        body: json!({
            "case_key": "repo:1055628515#1240@3",
            "repository_id": 1055628515_u64,
            "issue_number": 1240,
            "workflow_version": 3,
            "state_revision": 1,
            "role": "planner",
            "remediation_round": 0,
            "execution": "hermes",
            "provider": "openai-codex",
            "model": "gpt-5.6-sol",
            "projection_key": "repo:1055628515#1240@3:planner:round:1:revision:1:worker"
        })
        .to_string(),
    }
}

fn review_gate(status: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: "review-gate-1".into(),
        title: "Activate general-sol for repo:1055628515#1240@3".into(),
        status: status.into(),
        assignee: None,
        created_by: Some("pip-controller".into()),
        body: json!({
            "case_key": "repo:1055628515#1240@3",
            "state_revision": 2,
            "activation_gate": "general-sol",
            "projection_key": "repo:1055628515#1240@3:general-sol:round:1:revision:2:gate"
        })
        .to_string(),
    }
}

fn review_worker(status: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: "review-worker-1".into(),
        title: "Run general-sol for repo:1055628515#1240@3".into(),
        status: status.into(),
        assignee: Some("reviewer-general".into()),
        created_by: Some("pip-controller".into()),
        body: json!({
            "case_key": "repo:1055628515#1240@3",
            "repository_id": 1055628515_u64,
            "issue_number": 1240,
            "workflow_version": 3,
            "state_revision": 2,
            "role": "reviewer-general",
            "reviewer_id": "general-sol",
            "review_mode": "required",
            "remediation_round": 0,
            "review_round": 1,
            "execution": "hermes",
            "provider": "openai-codex",
            "model": "gpt-5.6-sol",
            "expected_head_sha": "c".repeat(40),
            "projection_key": "repo:1055628515#1240@3:general-sol:round:1:revision:2:worker"
        })
        .to_string(),
    }
}
