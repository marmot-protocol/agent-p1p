use std::cell::{Cell, RefCell};
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
    commands: Rc<RefCell<Vec<CommandSpec>>>,
    tasks: Rc<RefCell<Vec<TaskSnapshot>>>,
    fail_show: Rc<Cell<bool>>,
    fail_create_reply: Rc<Cell<bool>>,
    parents: Rc<RefCell<Vec<String>>>,
}

impl CommandRunner for FakeRunner {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput, HermesError> {
        self.commands.borrow_mut().push(command.clone());
        let flag = |name: &str| {
            command
                .args
                .windows(2)
                .find(|pair| pair[0] == name)
                .map(|pair| pair[1].clone())
                .unwrap()
        };
        let value = match command.args[3].as_str() {
            "list" => serde_json::to_value(&*self.tasks.borrow()).unwrap(),
            "create" => {
                assert!(
                    !command
                        .args
                        .iter()
                        .any(|arg| arg == "--parent" || arg == "--initial-status")
                );
                let workspace = flag("--workspace");
                let (kind, path) = workspace.split_once(':').unwrap();
                let task: TaskSnapshot = serde_json::from_value(json!({
                    "id": format!("worker-{}", self.tasks.borrow().len() + 1),
                    "title": command.args[4], "body": flag("--body"),
                    // Model the worker starting before the create reply arrives.
                    "status": "running", "assignee": flag("--assignee"),
                    "created_by": flag("--created-by"),
                    "workspace_kind": kind, "workspace_path": path,
                    "skills": command.args.windows(2).filter(|pair| pair[0] == "--skill").map(|pair| pair[1].clone()).collect::<Vec<_>>(),
                    "provider_override": flag("--provider"), "model_override": flag("--model"),
                    "max_retries": flag("--max-retries").parse::<u32>().unwrap(),
                    "priority": flag("--priority").parse::<u32>().unwrap()
                })).unwrap();
                self.tasks.borrow_mut().push(task.clone());
                if self.fail_create_reply.get() {
                    return Err(HermesError::TimedOut);
                }
                serde_json::to_value(task).unwrap()
            }
            "show" => {
                if self.fail_show.get() {
                    return Err(HermesError::CommandFailed(2));
                }
                let tasks = self.tasks.borrow();
                let task = tasks
                    .iter()
                    .find(|task| task.id == command.args[4])
                    .unwrap();
                json!({"task": task, "parents": *self.parents.borrow(), "runs": []})
            }
            other => panic!("unexpected command: {other}"),
        };
        Ok(CommandOutput {
            status: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: Vec::new(),
            timed_out: false,
        })
    }
}

#[derive(Clone, Default)]
struct FakeWorkspace {
    calls: Rc<RefCell<Vec<String>>>,
    fail: bool,
    fail_storage: bool,
}

impl WorkspacePreparer for FakeWorkspace {
    fn prepare_dispatch_storage(
        &self,
        _policy: &pip_control::RepositoryPolicy,
        store: &Store,
        _dispatches: &[pip_store::DispatchIntent],
    ) -> Result<(), WorkspaceError> {
        assert!(
            store
                .dispatch_intents("effect-intake-planner")
                .unwrap()
                .is_some()
        );
        if self.fail_storage {
            Err(WorkspaceError::InvalidCase)
        } else {
            Ok(())
        }
    }
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
fn missing_scratch_blocks_external_dispatch_after_freezing_intents() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let runner = FakeRunner::default();
    let workspace = FakeWorkspace {
        fail_storage: true,
        ..FakeWorkspace::default()
    };
    assert!(
        dispatch_once_with_workspace(
            &mut store,
            &active_policy(),
            runner.clone(),
            &workspace,
            context("controller-1", 100)
        )
        .is_err()
    );
    assert!(runner.commands.borrow().is_empty());
    assert_eq!(store.task_projection_count().unwrap(), 0);
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
fn planner_dispatch_freezes_intent_and_projects_only_worker_then_acks_outbox() {
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
            projection_count: 1,
            direct_job_count: 0,
            released_gate_count: 0,
            ledger_result: "applied".into(),
        }
    );
    let status = store.status(101).unwrap();
    assert_eq!(status.task_projections, 1);
    assert_eq!(status.outbox_delivered, 1);
    assert!(
        !runner
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
    runner.fail_show.set(true);
    assert!(
        dispatch_once_with(
            &mut store,
            &active_policy(),
            runner.clone(),
            context("controller-1", 100)
        )
        .is_err()
    );
    assert_eq!(store.status(101).unwrap().task_projections, 0);
    assert_eq!(store.status(101).unwrap().dispatch_batches, 1);
    assert!(
        store
            .claim_effect_matching(
                "direct-early",
                101,
                30,
                &["RUN_DIRECT_WORKER", "RUN_DIRECT_OBSERVER"]
            )
            .unwrap()
            .is_none()
    );
    runner.fail_show.set(false);
    runner.commands.borrow_mut().clear();

    assert_eq!(
        dispatch_once_with(
            &mut store,
            &active_policy(),
            runner.clone(),
            context("controller-2", 131),
        )
        .unwrap(),
        DispatchCycleResult::Projected {
            effect_id: "effect-dispatch-reviewers".into(),
            projection_count: 1,
            direct_job_count: 2,
            released_gate_count: 0,
            ledger_result: "applied".into(),
        }
    );
    assert!(runner.commands.borrow().iter().all(|command| {
        !command
            .args
            .windows(2)
            .any(|pair| pair[0] == "--provider" && pair[1] == "cursor")
    }));
    assert_eq!(runner.tasks.borrow().len(), 1);
    assert!(
        runner
            .commands
            .borrow()
            .iter()
            .all(|command| command.args[3] != "create")
    );
    let job = store
        .claim_effect_matching("direct-worker", 132, 30, &["RUN_DIRECT_WORKER"])
        .unwrap()
        .unwrap();
    assert_eq!(job.payload["role"], "reviewer-secperf");
    assert_eq!(job.payload["body"]["reviewer_id"], "secperf-kimi");
    assert_eq!(job.payload["model"], "kimi-k3-max");
    assert_eq!(job.payload["body"]["expected_head_sha"], "c".repeat(40));
    let shadow = store
        .claim_effect_matching("observer", 132, 30, &["RUN_DIRECT_OBSERVER"])
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
fn lost_create_reply_reconciles_running_or_done_task_without_redispatch() {
    for status in ["running", "done"] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = seeded_store(directory.path());
        let policy = active_policy();
        let first = creation_runner();
        first.fail_create_reply.set(true);
        assert!(
            dispatch_once_with(
                &mut store,
                &policy,
                first.clone(),
                context("controller-1", 100)
            )
            .is_err()
        );
        assert_eq!(store.task_projection_count().unwrap(), 0);
        assert!(
            store
                .dispatch_intents("effect-intake-planner")
                .unwrap()
                .is_some()
        );
        first.tasks.borrow_mut()[0].status = status.into();
        let recovered = FakeRunner {
            tasks: first.tasks.clone(),
            ..FakeRunner::default()
        };
        let report = dispatch_once_with(
            &mut store,
            &policy,
            recovered.clone(),
            context("controller-2", 131),
        )
        .unwrap();
        assert!(matches!(report, DispatchCycleResult::Projected { .. }));
        assert_eq!(store.task_projection_count().unwrap(), 1);
        assert!(
            recovered
                .commands
                .borrow()
                .iter()
                .all(|command| command.args[3] != "create")
        );
    }
}

#[test]
fn deleted_or_archived_uncertain_task_never_causes_a_second_create() {
    for archived in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = seeded_store(directory.path());
        let first = creation_runner();
        first.fail_show.set(true);
        assert!(
            dispatch_once_with(
                &mut store,
                &active_policy(),
                first.clone(),
                context("controller-1", 100)
            )
            .is_err()
        );
        let recovered = FakeRunner {
            tasks: first.tasks.clone(),
            ..FakeRunner::default()
        };
        if archived {
            recovered.tasks.borrow_mut()[0].status = "archived".into();
        } else {
            recovered.tasks.borrow_mut().clear();
        }
        assert!(
            dispatch_once_with(
                &mut store,
                &active_policy(),
                recovered.clone(),
                context("controller-2", 131)
            )
            .is_err()
        );
        assert!(
            recovered
                .commands
                .borrow()
                .iter()
                .all(|command| command.args[3] != "create")
        );
        assert_eq!(store.status(132).unwrap().outbox_delivered, 0);
    }
}

#[test]
fn unexpected_dependencies_prevent_acknowledgment() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let runner = creation_runner();
    runner.parents.borrow_mut().push("foreign-parent".into());
    assert!(
        dispatch_once_with(
            &mut store,
            &active_policy(),
            runner,
            context("controller-1", 100)
        )
        .is_err()
    );
    assert_eq!(store.status(101).unwrap().outbox_delivered, 0);
}

#[test]
fn recovery_uses_saved_job_when_release_or_profile_defaults_change() {
    for change_model in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = seeded_store(directory.path());
        let first = creation_runner();
        first.fail_show.set(true);
        assert!(
            dispatch_once_with(
                &mut store,
                &active_policy(),
                first.clone(),
                context("controller-1", 100)
            )
            .is_err()
        );
        let mut policy = active_policy();
        let mut retry = context("controller-2", 131);
        if change_model {
            policy
                .roles
                .iter_mut()
                .find(|role| role.profile == "planner")
                .unwrap()
                .model = "another-model".into();
        } else {
            retry.skills_repository_commit = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        }
        let original = first.tasks.borrow()[0].clone();
        let frozen = store.dispatch_intents("effect-intake-planner").unwrap();
        first.fail_show.set(false);
        assert!(matches!(
            dispatch_once_with(&mut store, &policy, first.clone(), retry).unwrap(),
            DispatchCycleResult::Projected {
                projection_count: 1,
                ..
            }
        ));
        assert_eq!(*first.tasks.borrow(), vec![original]);
        assert_eq!(
            store.dispatch_intents("effect-intake-planner").unwrap(),
            frozen
        );
        assert_eq!(
            first
                .commands
                .borrow()
                .iter()
                .filter(|c| c.args[3] == "create")
                .count(),
            1
        );
    }
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
    FakeRunner::default()
}

fn review_creation_runner() -> FakeRunner {
    FakeRunner::default()
}

/// Opt-in compatibility test against a stock installed Hermes. It ALWAYS
/// creates its own temporary home, board, ledger, profile, and workspace.
/// The injected spawn callback is Hermes's supported test seam; no provider or
/// production gateway is started, and installed Hermes source is never edited.
#[test]
#[ignore = "requires PIP_TEST_HERMES and PIP_TEST_HERMES_PYTHON pointing to a stock installation"]
fn real_hermes_cli_and_dispatcher_recover_a_lost_create_reply() {
    use pip_hermes::{HermesReader, ProcessRunner};
    use std::time::Duration;
    let hermes = std::env::var("PIP_TEST_HERMES").expect("explicit Hermes executable");
    let python =
        std::env::var("PIP_TEST_HERMES_PYTHON").expect("explicit Hermes Python executable");
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let home = root.join("hermes");
    std::fs::create_dir_all(home.join("home")).unwrap();
    let mut store = seeded_store(&root);
    let mut policy = active_policy();
    policy.board = "pip-isolated-dispatch-test".into();
    policy.workspace = root.join("workspaces").display().to_string();
    let expected_workspace = format!("{}/repo-1055628515-issue-1240-workflow-3", policy.workspace);
    std::fs::create_dir_all(&expected_workspace).unwrap();
    let real = ProcessRunner::for_hermes_root(&home).unwrap();
    let run = |program: &str, args: Vec<String>| {
        let output = real
            .run(&CommandSpec {
                program: program.into(),
                args,
                timeout: Duration::from_secs(30),
                max_output_bytes: 1024 * 1024,
            })
            .unwrap();
        assert!(!output.timed_out);
        assert_eq!(
            output.status,
            0,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    run(
        &hermes,
        vec![
            "kanban".into(),
            "boards".into(),
            "create".into(),
            policy.board.clone(),
        ],
    );
    #[derive(Clone)]
    struct LoseCreateReply(ProcessRunner);
    impl CommandRunner for LoseCreateReply {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
            let output = self.0.run(spec)?;
            if spec.args.get(3).is_some_and(|arg| arg == "create") {
                assert_eq!(
                    output.status,
                    0,
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return Err(HermesError::TimedOut);
            }
            Ok(output)
        }
    }
    let mut initial = context("isolated-first", 100);
    initial.hermes_program = &hermes;
    assert!(matches!(
        dispatch_once_with(&mut store, &policy, LoseCreateReply(real.clone()), initial),
        Err(DispatchCycleError::Projection(_))
    ));
    let reader =
        HermesReader::new(real.clone(), &hermes, Duration::from_secs(30), 1024 * 1024).unwrap();
    let tasks = reader.list_tasks(&policy.board).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "ready");
    assert_eq!(
        tasks[0].configuration.workspace_kind.as_deref(),
        Some("dir")
    );
    assert_eq!(
        tasks[0].configuration.workspace_path.as_deref(),
        Some(expected_workspace.as_str())
    );

    let probe = r#"
import json, pathlib, sys
from hermes_cli import kanban_db as kb
from hermes_cli.profiles import get_profile_dir
board, task_id, workspace = sys.argv[1:]
get_profile_dir('planner').mkdir(parents=True, exist_ok=True)
with kb.connect_closing(board=board) as conn:
    def deterministic_worker(task, workspace_path, board=None):
        assert task.id == task_id
        assert kb.get_task(conn, task_id).status == 'running'
        assert pathlib.Path(workspace_path) == pathlib.Path(workspace)
        assert kb.complete_task(conn, task_id, result='isolated-no-model-completion', fire_lifecycle_hook=False)
        return None
    result = kb.dispatch_once(conn, board=board, spawn_fn=deterministic_worker, max_spawn=1)
    assert len(result.spawned) == 1, repr(result)
    assert kb.get_task(conn, task_id).status == 'done'
    assert kb.parent_ids(conn, task_id) == []
    print(json.dumps({'spawned': len(result.spawned), 'status': 'done'}))
"#;
    run(
        &python,
        vec![
            "-c".into(),
            probe.into(),
            policy.board.clone(),
            tasks[0].id.clone(),
            expected_workspace,
        ],
    );
    let mut recovery = context("isolated-recovery", 200);
    recovery.hermes_program = &hermes;
    let report = dispatch_once_with(&mut store, &policy, real, recovery).unwrap();
    assert!(matches!(
        report,
        DispatchCycleResult::Projected {
            projection_count: 1,
            released_gate_count: 0,
            ..
        }
    ));
    let tasks = reader.list_tasks(&policy.board).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, "done");
    assert_eq!(store.status(201).unwrap().outbox_delivered, 1);
}
