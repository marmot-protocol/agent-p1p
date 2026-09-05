use pip_control::{
    RepositoryPolicy, load_repository_policy, prepare_hermes_scratch, retire_hermes_scratch,
};
use pip_store::{EventInput, NewCase, Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn offline_retirement_gate_requires_stopped_disabled_execution_units() {
    struct Runner(&'static str);
    impl pip_hermes::CommandRunner for Runner {
        fn run(
            &self,
            spec: &pip_hermes::CommandSpec,
        ) -> Result<pip_hermes::CommandOutput, pip_hermes::HermesError> {
            assert_eq!(spec.program, "systemctl");
            Ok(pip_hermes::CommandOutput {
                status: 0,
                stdout: if spec.args[0] == "show" {
                    b"0\n".to_vec()
                } else {
                    self.0.as_bytes().to_vec()
                },
                stderr: vec![],
                timed_out: false,
            })
        }
    }
    assert!(pip_control::verify_scratch_runtime_stopped(&Runner("")).is_ok());
    assert!(
        pip_control::verify_scratch_runtime_stopped(&Runner(
            "pip-controller@mdk.timer loaded active waiting"
        ))
        .is_err()
    );
}

fn setup(state: &str) -> (tempfile::TempDir, RepositoryPolicy, Store, Value) {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    for child in ["worktrees", "scratch"] {
        fs::create_dir(base.join(child)).unwrap();
    }
    fs::set_permissions(base.join("scratch"), fs::Permissions::from_mode(0o700)).unwrap();
    let mut policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy.workspace = base.join("worktrees").display().to_string();
    policy.hermes_scratch_root = Some(base.join("scratch").display().to_string());
    policy.workspace_storage.minimum_free_bytes = 1;
    policy.workspace_storage.require_distinct_filesystem = false;
    policy.workspace_storage.terminal_retention_seconds = 10;
    let mut store = Store::open(base.join("ledger.db")).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:1055628515#42@3".into(),
            repository_id: 1055628515,
            issue_number: 42,
            workflow_version: 3,
            policy_revision: policy.revision,
            initial_state: state.into(),
            observed_at: 100,
            event: EventInput {
                event_id: "fixture".into(),
                event_type: "FIXTURE".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    let key = "repo:1055628515#42@3:planner:worker";
    let hash: String = Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let root = format!("{}/{hash}", policy.hermes_scratch_root.as_ref().unwrap());
    let body = json!({"case_key":"repo:1055628515#42@3", "projection_key":key,
        "storage": {"schema_version":1,"root":root,"source":format!("{}/repo-1055628515-issue-42-workflow-3",policy.workspace),
            "cargo_target":format!("{root}/disposable/target"),"cargo_home":format!("{root}/disposable/cargo-home"),"temporary":format!("{root}/disposable/tmp"),"results":format!("{root}/results")}});
    (temp, policy, store, body)
}

#[test]
fn scratch_replays_without_erasing_results_and_retirement_requires_quiescent_terminal_case() {
    let (_temp, policy, store, body) = setup("COMPLETED");
    prepare_hermes_scratch(&policy, &store, &body).unwrap();
    let root = std::path::Path::new(body["storage"]["root"].as_str().unwrap());
    fs::write(root.join("disposable/target/large-build"), "build").unwrap();
    fs::write(root.join("results/plan.json"), "retain").unwrap();
    prepare_hermes_scratch(&policy, &store, &body).unwrap();
    assert!(retire_hermes_scratch(&policy, &store, &body, 200, false).is_err());
    assert!(retire_hermes_scratch(&policy, &store, &body, 105, true).is_err());
    retire_hermes_scratch(&policy, &store, &body, 200, true).unwrap();
    assert!(!root.join("disposable").exists());
    assert_eq!(
        fs::read_to_string(root.join("results/plan.json")).unwrap(),
        "retain"
    );
    retire_hermes_scratch(&policy, &store, &body, 200, true).unwrap();
}

#[test]
fn scratch_refuses_active_cases_low_space_unowned_paths_and_symlink_roots() {
    let (temp, mut policy, store, body) = setup("PLANNING");
    policy.workspace_storage.minimum_free_bytes = u64::MAX;
    assert!(prepare_hermes_scratch(&policy, &store, &body).is_err());
    assert!(!std::path::Path::new(body["storage"]["root"].as_str().unwrap()).exists());
    policy.workspace_storage.minimum_free_bytes = 1;
    let mut foreign = body.clone();
    foreign["storage"]["cargo_target"] = json!(temp.path());
    assert!(prepare_hermes_scratch(&policy, &store, &foreign).is_err());
    let root = std::path::Path::new(body["storage"]["root"].as_str().unwrap());
    symlink(temp.path(), root).unwrap();
    assert!(prepare_hermes_scratch(&policy, &store, &body).is_err());
    fs::remove_file(root).unwrap();
    prepare_hermes_scratch(&policy, &store, &body).unwrap();
    assert!(retire_hermes_scratch(&policy, &store, &body, 200, true).is_err());
}
