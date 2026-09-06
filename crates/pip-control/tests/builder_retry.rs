use pip_control::{
    BuilderRetryRequest, DispatchCycleContext, OperationalBound, OperationalBoundsCycle,
    RepositoryPolicy, authorize_builder_retry, dispatch_once_with, enforce_operational_bounds,
    load_repository_policy,
};
use pip_hermes::{CommandOutput, CommandRunner, CommandSpec, HermesError};
use pip_store::{
    ApplyResult, EffectInput, EventInput, NewCase, PolicyInput, RunInput, Store, TransitionInput,
};
use serde_json::json;

const CASE: &str = "repo:42#9@3";
#[derive(Clone)]
struct NoHermes;
impl CommandRunner for NoHermes {
    fn run(&self, _: &CommandSpec) -> Result<CommandOutput, HermesError> {
        panic!("builder retry must not invoke Hermes")
    }
}

fn fixture() -> (
    tempfile::TempDir,
    Store,
    RepositoryPolicy,
    RepositoryPolicy,
    BuilderRetryRequest,
) {
    fixture_at(100)
}

fn fixture_at(
    created: u64,
) -> (
    tempfile::TempDir,
    Store,
    RepositoryPolicy,
    RepositoryPolicy,
    BuilderRetryRequest,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("ledger.db")).unwrap();
    let mut paused = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    paused.repository.id = 42;
    let mut accepted = paused.clone();
    accepted.revision = 7;
    accepted.intake.enabled = true;
    accepted.intake.paused = false;
    accepted.dispatch_enabled = true;
    store
        .record_policy(&PolicyInput {
            repository_id: 42,
            revision: 7,
            accepted_at: created,
            payload: serde_json::to_value(&accepted).unwrap(),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 42,
            issue_number: 9,
            workflow_version: 3,
            policy_revision: 7,
            initial_state: "PLANNING".into(),
            observed_at: created,
            event: EventInput {
                event_id: "intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: 1,
                next_state: "READY_TO_BUILD".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: created + 1,
                event: EventInput {
                    event_id: "plan".into(),
                    event_type: "PROCEED".into(),
                    payload: json!({}),
                },
                run: Some(RunInput {
                    run_id: "plan-run".into(),
                    task_id: "planner".into(),
                    role: "planner".into(),
                    payload: json!({"plan_version":1,"plan_sha256":"a".repeat(64)}),
                }),
                evidence: vec![],
                findings: vec![],
                effects: vec![EffectInput {
                    effect_id: "old-builder".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: json!({"role":"builder","task_id":"old-task"}),
                }],
            },
            None,
        )
        .unwrap();
    for now in [created + 10, created + 20, created + 30] {
        let claimed = store.claim_effect("worker", now, 5).unwrap().unwrap();
        let id = store
            .begin_direct_attempt(&claimed, "old-task", now)
            .unwrap();
        store
            .fail_direct_attempt(id, "worker", now + 1, "startup failure")
            .unwrap();
        store.release_effect(&claimed.effect_id, "worker").unwrap();
    }
    let request = BuilderRetryRequest {
        case_key: CASE.into(),
        expected_revision: 2,
        effect_id: "old-builder".into(),
        expected_failures: 3,
        request_id: "operator-retry-1".into(),
        reason: "Workspace boundary repaired".into(),
    };
    (dir, store, paused, accepted, request)
}

#[test]
fn retry_redispatches_exact_model_with_fresh_skills_and_stops_after_another_failure() {
    let (_dir, mut store, paused, accepted, request) = fixture();
    let history = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 140, 0).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 141, 0).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(
        enforce_operational_bounds(&mut store, &accepted, 142).unwrap(),
        OperationalBoundsCycle::Idle
    );
    dispatch_once_with(
        &mut store,
        &accepted,
        NoHermes,
        DispatchCycleContext {
            skills_repository_commit: &"b".repeat(40),
            hermes_program: "unused",
            owner: "controller",
            now: 143,
            lease_seconds: 30,
            authorization_valid: true,
        },
    )
    .unwrap();
    let mut stacked = request.clone();
    stacked.request_id = "operator-retry-2".into();
    stacked.expected_revision = 3;
    let before = store.status(143).unwrap();
    assert!(authorize_builder_retry(&mut store, &paused, &stacked, 143, 0).is_err());
    assert_eq!(store.status(143).unwrap(), before);
    assert_eq!(
        store
            .effective_provider_failure_limit("repo:42#10@3", 3)
            .unwrap(),
        3
    );
    let claimed = store
        .claim_effect_matching("worker", 144, 30, &["RUN_DIRECT_WORKER"])
        .unwrap()
        .unwrap();
    assert_ne!(claimed.effect_id, "old-builder");
    assert_eq!(
        claimed.payload["body"]["skills_repository_commit"],
        "b".repeat(40)
    );
    assert_eq!(claimed.payload["body"]["plan_version"], 1);
    assert_eq!(
        claimed.payload["model"],
        accepted
            .roles
            .iter()
            .find(|r| r.role == pip_contracts::WorkerRole::Builder)
            .unwrap()
            .model
    );
    assert_eq!(claimed.state_revision, 3);
    let id = store
        .begin_direct_attempt(&claimed, claimed.payload["task_id"].as_str().unwrap(), 144)
        .unwrap();
    store
        .fail_direct_attempt(id, "worker", 145, "new failure")
        .unwrap();
    store.release_effect(&claimed.effect_id, "worker").unwrap();
    assert_eq!(store.status(145).unwrap().direct_attempts_failed, 4);
    assert_eq!(
        store.immutable_history_for_case(CASE).unwrap().runs,
        history.runs
    );
    assert!(matches!(
        enforce_operational_bounds(&mut store, &accepted, 146).unwrap(),
        OperationalBoundsCycle::Escalated {
            bound: OperationalBound::ProviderFailures,
            observed: 4,
            limit: 4,
            ..
        }
    ));
    let after = store.status(147).unwrap();
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 147, 0).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(store.status(147).unwrap(), after);
}

#[test]
fn retry_requires_root_inert_runtime_bindings_and_exact_request() {
    for scenario in [
        "uid", "active", "model", "revision", "effect", "reason", "time",
    ] {
        let (_dir, mut store, mut paused, _accepted, mut request) = fixture();
        let mut uid = 0;
        let mut now = 140;
        match scenario {
            "uid" => uid = 1000,
            "active" => paused.dispatch_enabled = true,
            "model" => paused.roles[1].model = "different".into(),
            "revision" => request.expected_revision = 1,
            "effect" => request.effect_id = "foreign".into(),
            "reason" => request.reason.clear(),
            "time" => now = 86500,
            _ => unreachable!(),
        }
        let before = store.status(140).unwrap();
        assert!(
            authorize_builder_retry(&mut store, &paused, &request, now, uid).is_err(),
            "{scenario}"
        );
        assert_eq!(store.status(140).unwrap(), before);
    }
    let (_dir, mut store, paused, _accepted, mut request) = fixture();
    authorize_builder_retry(&mut store, &paused, &request, 140, 0).unwrap();
    request.reason = "changed authorization".into();
    assert!(authorize_builder_retry(&mut store, &paused, &request, 141, 0).is_err());
}

#[test]
fn retry_never_extends_the_case_deadline() {
    let (_dir, mut store, paused, accepted, request) = fixture();
    authorize_builder_retry(&mut store, &paused, &request, 140, 0).unwrap();
    assert!(matches!(
        enforce_operational_bounds(&mut store, &accepted, 86500).unwrap(),
        OperationalBoundsCycle::Escalated {
            bound: OperationalBound::ElapsedTime,
            ..
        }
    ));
}

#[test]
#[ignore = "requires root and disposable systemd; run by the lifecycle harness"]
fn root_cli_retry_checks_real_uid_stopped_units_and_empty_queue() {
    use std::{fs, process::Command};
    assert!(rustix::process::geteuid().is_root());
    assert!(std::path::Path::new("/run/systemd/system").is_dir());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (dir, store, paused, _accepted, _request) = fixture_at(now - 40);
    let policy = dir.path().join("paused.json");
    fs::write(&policy, serde_json::to_vec(&paused).unwrap()).unwrap();
    let queue = dir.path().join("queue");
    for name in ["inbox", "results", "archive"] {
        fs::create_dir_all(queue.join(name)).unwrap();
    }
    let args = [
        "authorize-builder-retry",
        "--policy",
        policy.to_str().unwrap(),
        "--database",
        store.path().to_str().unwrap(),
        "--direct-queue",
        queue.to_str().unwrap(),
        "--case",
        CASE,
        "--expected-revision",
        "2",
        "--effect-id",
        "old-builder",
        "--expected-failures",
        "3",
        "--request-id",
        "operator-retry-1",
        "--reason",
        "Workspace repaired",
    ];
    let binary = env!("CARGO_BIN_EXE_pip-control");
    let run = || Command::new(binary).args(args).output().unwrap();
    let denied = Command::new("runuser")
        .args(["-u", "pip-worker", "--", binary])
        .args(args)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("requires root"));
    fs::write(queue.join("inbox/stale.json"), b"{}").unwrap();
    let stale = run();
    assert!(!stale.status.success());
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("queue must be drained"),
        "{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    fs::remove_file(queue.join("inbox/stale.json")).unwrap();
    assert!(
        Command::new("systemctl")
            .args(["enable", "--runtime", "pip-controller@mdk.timer"])
            .status()
            .unwrap()
            .success()
    );
    let active = run();
    assert!(
        Command::new("systemctl")
            .args(["disable", "--runtime", "pip-controller@mdk.timer"])
            .status()
            .unwrap()
            .success()
    );
    assert!(!active.status.success());
    assert!(String::from_utf8_lossy(&active.stderr).contains("execution units"));
    assert_eq!(store.case(CASE).unwrap().unwrap().state_revision, 2);
    for expected in ["Applied", "Replayed"] {
        let output = run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["result"], expected);
        assert_eq!(value["runtime_activated"], false);
    }
    assert_eq!(store.status(140).unwrap().direct_attempts_running, 0);
    assert_eq!(store.case(CASE).unwrap().unwrap().state_revision, 3);
}
