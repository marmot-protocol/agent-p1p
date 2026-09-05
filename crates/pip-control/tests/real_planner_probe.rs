//! Operator-only, two-phase real-provider probe. Preparation queues exactly one
//! worker; execution belongs in a bounded OS sandbox, outside this test process.
//! Verification reads the worker's durable metadata without repairing it.
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pip_control::{
    DispatchCycleContext, DispatchCycleResult, ResultCycle, bootstrap_hermes_runtime_with,
    dispatch_once_with, ingest_completed_once_with, load_repository_policy,
};
use pip_hermes::{HermesReader, ProcessRunner};
use pip_store::{EffectInput, EventInput, NewCase, PolicyInput, Store};
use serde_json::json;

#[test]
#[ignore = "explicit isolated root, Hermes executable, skills commit, and prepare/verify phase required"]
fn isolated_real_planner_contract() {
    let root = PathBuf::from(std::env::var("PIP_PLANNER_PROBE_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("ISOLATED_PROBE")).unwrap(),
        "pip-real-planner-probe\n"
    );
    let phase = std::env::var("PIP_PLANNER_PROBE_PHASE").unwrap();
    assert!(matches!(phase.as_str(), "prepare" | "verify"));
    let hermes = std::env::var("PIP_TEST_HERMES").unwrap();
    let commit = std::env::var("PIP_TEST_SKILLS_COMMIT").unwrap();
    let home = root.join("hermes");
    if phase == "prepare" {
        assert!(!root.join("policy.json").exists());
        let mut policy: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../config/target/repositories/mdk.json"
        ))
        .unwrap();
        policy["repository"] = json!({"id": 17, "owner": "pip-fixture", "name": "local-only", "default_branch": "master"});
        policy["board"] = json!("pip-isolated-planner-probe");
        policy["workspace"] = json!(root.join("workspaces"));
        policy["checkout"] = json!(root.join("fixture"));
        policy["artifacts"] = json!(root.join("artifacts"));
        policy["intake"]["enabled"] = json!(true);
        policy["intake"]["paused"] = json!(false);
        policy["dispatch_enabled"] = json!(true);
        policy["github"]["automation_actor_id"] = json!(101);
        policy["github"]["reviewer_general_actor_id"] = json!(102);
        policy["github"]["reviewer_secperf_actor_id"] = json!(103);
        std::fs::write(
            root.join("policy.json"),
            serde_json::to_vec_pretty(&policy).unwrap(),
        )
        .unwrap();
    }
    let policy = load_repository_policy(&std::fs::read(root.join("policy.json")).unwrap()).unwrap();
    // No production identity, board, or path can be reused by this probe.
    assert_eq!(policy.repository.id, 17);
    assert_eq!(policy.repository.full_name(), "pip-fixture/local-only");
    assert_eq!(policy.board, "pip-isolated-planner-probe");
    assert_eq!(policy.workflow_version, 3);
    assert_eq!(PathBuf::from(&policy.workspace), root.join("workspaces"));
    assert!(policy.merge.is_shadow());
    assert!(!policy.merge.autonomous);
    let workspace = root.join("workspaces/repo-17-issue-1-workflow-3");
    let runner = ProcessRunner::for_hermes_root(&home).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let reader = HermesReader::new(
        runner.clone(),
        &hermes,
        Duration::from_secs(30),
        4 * 1024 * 1024,
    )
    .unwrap();
    if phase == "prepare" {
        assert!(
            !root.join("ledger.db").exists(),
            "never overwrite or requeue a probe"
        );
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::create_dir_all(workspace.join("docs")).unwrap();
        for path in ["AGENTS.md", "issue.json", "src/lib.rs"] {
            std::fs::copy(
                root.join("source/tests/fixtures/planner-probe").join(path),
                workspace.join(path),
            )
            .unwrap();
        }
        std::fs::copy(
            root.join(
                "source/skills/shared/workflow-contract/references/worker-result-contracts.md",
            ),
            workspace.join("docs/worker-result-contracts.md"),
        )
        .unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(&workspace)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        git(&["init", "--initial-branch=master"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Pip Fixture",
            "-c",
            "user.email=fixture@invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "Immutable local planner fixture",
        ]);
        std::fs::write(root.join("fixture-head"), git(&["rev-parse", "HEAD"])).unwrap();
        std::fs::create_dir(&home).unwrap();
        std::fs::copy(root.join("credential/auth.json"), home.join("auth.json")).unwrap();
        bootstrap_hermes_runtime_with(
            &policy,
            &home,
            &root.join("source/skills"),
            &home.join("auth.json"),
            &hermes,
            runner.clone(),
        )
        .unwrap();
        assert!(reader.list_tasks(&policy.board).unwrap().is_empty());
        let mut store = Store::open(root.join("ledger.db")).unwrap();
        store
            .record_policy(&PolicyInput {
                repository_id: 17,
                revision: policy.revision,
                accepted_at: now,
                payload: serde_json::to_value(&policy).unwrap(),
            })
            .unwrap();
        let issue: serde_json::Value =
            serde_json::from_slice(&std::fs::read(workspace.join("issue.json")).unwrap()).unwrap();
        store.create_case(&NewCase {
            case_key: "repo:17#1@3".into(), repository_id: 17, issue_number: 1,
            workflow_version: 3, policy_revision: policy.revision,
            initial_state: "PLANNING".into(), observed_at: now,
            event: EventInput {
                event_id: "isolated-fixture-authorized".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"fixture_only": true, "issue": issue,
                    "instructions": "Offline local fixture, not a GitHub issue. Use the supplied immutable checkout and issue.json; do not fetch or publish. Read AGENTS.md and docs/worker-result-contracts.md. Produce the real planner contract and submit it via kanban_complete metadata. Do not implement the fix."}),
            },
            effects: vec![EffectInput {
                effect_id: "isolated-planner-dispatch".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key": "repo:17#1@3"}),
            }],
        }).unwrap();
        let report = dispatch_once_with(
            &mut store,
            &policy,
            runner,
            DispatchCycleContext {
                skills_repository_commit: &commit,
                hermes_program: &hermes,
                owner: "isolated-probe",
                now,
                lease_seconds: 120,
                authorization_valid: true,
            },
        )
        .unwrap();
        assert!(matches!(
            report,
            DispatchCycleResult::Projected {
                projection_count: 1,
                direct_job_count: 0,
                released_gate_count: 0,
                ..
            }
        ));
        let tasks = reader.list_tasks(&policy.board).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "ready");
        std::fs::write(root.join("task-id"), &tasks[0].id).unwrap();
        println!("ISOLATED_PLANNER_QUEUED {}", tasks[0].id);
    } else {
        let mut store = Store::open(root.join("ledger.db")).unwrap();
        assert_eq!(store.run_count().unwrap(), 0, "verification is single-use");
        let tasks = reader.list_tasks(&policy.board).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].id,
            std::fs::read_to_string(root.join("task-id")).unwrap()
        );
        let completed = reader
            .show_completed_result(&policy.board, &tasks[0].id)
            .unwrap();
        // Preserve exact evidence even if Rust subsequently rejects it.
        std::fs::write(
            root.join("worker-result.json"),
            serde_json::to_vec_pretty(&completed.metadata).unwrap(),
        )
        .unwrap();
        assert_eq!(completed.metadata["outcome"], "PROCEED");
        let base = std::fs::read_to_string(root.join("fixture-head")).unwrap();
        assert_eq!(completed.metadata["planned_base_sha"], base.trim());
        let artifact = PathBuf::from(completed.metadata["plan_artifact"].as_str().unwrap());
        let artifact = if artifact.is_absolute() {
            artifact
        } else {
            workspace.join(artifact)
        };
        let artifact = artifact.canonicalize().unwrap();
        assert!(artifact.starts_with(workspace.canonicalize().unwrap()));
        let contents = std::fs::read_to_string(artifact).unwrap();
        assert!(contents.contains(base.trim()));
        assert!(contents.len() > 200);
        let plan_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(workspace.join("plans/plan-v1.json")).unwrap())
                .unwrap();
        assert_eq!(completed.worker_contract_metadata().unwrap(), plan_json);
        let report =
            ingest_completed_once_with(&mut store, &policy, runner.clone(), &hermes, now).unwrap();
        assert!(matches!(
            report,
            ResultCycle::Ingested {
                transition_count: 1,
                ..
            }
        ));
        assert_eq!(store.run_count().unwrap(), 1);
        // The acceptance schedules publication, but never executes it here.
        assert_eq!(
            store.case("repo:17#1@3").unwrap().unwrap().state,
            "PLANNING"
        );
        assert_eq!(store.status(now).unwrap().outbox_pending, 1);
        assert_eq!(
            ingest_completed_once_with(&mut store, &policy, runner, &hermes, now,).unwrap(),
            ResultCycle::Idle
        );
        println!("ISOLATED_REAL_PLANNER_CONTRACT_ACCEPTED {}", tasks[0].id);
    }
}
