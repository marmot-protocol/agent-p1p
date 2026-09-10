use std::cell::RefCell;
use std::path::Path;

use pip_control::{
    RepositoryPolicy, WorkspaceLifecycleError, WorkspaceRetirement, WorkspaceStorageProbe,
    WorkspaceStorageSnapshot, load_repository_policy, reconcile_workspace_lifecycle_once_with,
};
use pip_executor::{AllocationError, RetirementResult, WorktreeRetirementSpec};
use pip_store::{ApplyResult, EventInput, NewCase, Store};
use serde_json::json;

struct SequenceProbe {
    snapshots: RefCell<Vec<WorkspaceStorageSnapshot>>,
}

impl WorkspaceStorageProbe for SequenceProbe {
    fn inspect(
        &self,
        _workspace: &Path,
        _ledger: &Path,
    ) -> Result<WorkspaceStorageSnapshot, WorkspaceLifecycleError> {
        Ok(self.snapshots.borrow_mut().remove(0))
    }
}

#[derive(Default)]
struct RecordingRetirer {
    paths: RefCell<Vec<String>>,
}

impl WorkspaceRetirement for RecordingRetirer {
    fn retire(&self, spec: &WorktreeRetirementSpec) -> Result<RetirementResult, AllocationError> {
        self.paths
            .borrow_mut()
            .push(spec.path().to_string_lossy().into_owned());
        Ok(RetirementResult::Retired)
    }
}

fn policy(directory: &tempfile::TempDir, minimum_free_bytes: u64) -> RepositoryPolicy {
    let checkout = directory.path().join("repo");
    let workspace = directory.path().join("worktrees");
    let artifacts = directory.path().join("artifacts");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&artifacts).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["checkout"] = json!(checkout);
    value["workspace"] = json!(workspace);
    value["artifacts"] = json!(artifacts);
    value["workspace_storage"]["require_distinct_filesystem"] = json!(true);
    value["workspace_storage"]["minimum_free_bytes"] = json!(minimum_free_bytes);
    value["workspace_storage"]["terminal_retention_seconds"] = json!(100);
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn terminal_case(policy_revision: u64) -> NewCase {
    NewCase {
        case_key: "repo:1055628515#1240@2".into(),
        repository_id: 1_055_628_515,
        issue_number: 1240,
        workflow_version: 2,
        policy_revision,
        initial_state: "COMPLETED".into(),
        observed_at: 100,
        event: EventInput {
            event_id: "terminal-intake".into(),
            event_type: "FIXTURE".into(),
            payload: json!({}),
        },
        effects: vec![],
    }
}

#[test]
fn busy_native_review_defers_retirement_without_blocking_other_work() {
    let directory = tempfile::tempdir().unwrap();
    let policy = policy(&directory, 500);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store.create_case(&terminal_case(policy.revision)).unwrap();
    let probe = SequenceProbe {
        snapshots: RefCell::new(vec![WorkspaceStorageSnapshot {
            free_bytes: 1000,
            distinct_filesystem: true,
        }]),
    };
    let retirer = RecordingRetirer::default();
    let cycle = pip_control::reconcile_workspace_lifecycle_with_quiescence(
        &mut store,
        &policy,
        300,
        &probe,
        &retirer,
        |key, _| {
            assert_eq!(key, "repo:1055628515#1240@2");
            false
        },
    )
    .unwrap();
    assert!(cycle.ready);
    assert!(cycle.retired_case_key.is_none());
    assert!(retirer.paths.borrow().is_empty());
    assert_eq!(store.status(300).unwrap().workspace_retirements, 0);
}

#[test]
fn lifecycle_retires_one_retained_terminal_worktree_and_rechecks_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let policy = policy(&directory, 500);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    assert_eq!(
        store.create_case(&terminal_case(policy.revision)).unwrap(),
        ApplyResult::Applied
    );
    let probe = SequenceProbe {
        snapshots: RefCell::new(vec![
            WorkspaceStorageSnapshot {
                free_bytes: 100,
                distinct_filesystem: true,
            },
            WorkspaceStorageSnapshot {
                free_bytes: 700,
                distinct_filesystem: true,
            },
        ]),
    };
    let retirer = RecordingRetirer::default();

    let cycle = reconcile_workspace_lifecycle_once_with(&mut store, &policy, 300, &probe, &retirer)
        .unwrap();

    assert!(cycle.ready);
    assert_eq!(cycle.free_bytes, 700);
    assert_eq!(
        cycle.retired_case_key.as_deref(),
        Some("repo:1055628515#1240@2")
    );
    assert_eq!(retirer.paths.borrow().len(), 1);
    assert_eq!(store.status(300).unwrap().workspace_retirements, 1);
}

#[test]
fn lifecycle_fails_closed_on_the_wrong_filesystem_before_retirement() {
    let directory = tempfile::tempdir().unwrap();
    let policy = policy(&directory, 500);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store.create_case(&terminal_case(policy.revision)).unwrap();
    let probe = SequenceProbe {
        snapshots: RefCell::new(vec![WorkspaceStorageSnapshot {
            free_bytes: 1_000,
            distinct_filesystem: false,
        }]),
    };
    let retirer = RecordingRetirer::default();

    assert!(matches!(
        reconcile_workspace_lifecycle_once_with(&mut store, &policy, 300, &probe, &retirer),
        Err(WorkspaceLifecycleError::SharedFilesystem)
    ));
    assert!(retirer.paths.borrow().is_empty());
    assert_eq!(store.status(300).unwrap().workspace_retirements, 0);
}

#[test]
fn lifecycle_reports_low_capacity_without_retiring_recent_cases() {
    let directory = tempfile::tempdir().unwrap();
    let policy = policy(&directory, 500);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store.create_case(&terminal_case(policy.revision)).unwrap();
    let probe = SequenceProbe {
        snapshots: RefCell::new(vec![WorkspaceStorageSnapshot {
            free_bytes: 100,
            distinct_filesystem: true,
        }]),
    };
    let retirer = RecordingRetirer::default();

    let cycle = reconcile_workspace_lifecycle_once_with(&mut store, &policy, 150, &probe, &retirer)
        .unwrap();
    assert!(!cycle.ready);
    assert_eq!(cycle.free_bytes, 100);
    assert!(cycle.retired_case_key.is_none());
    assert!(retirer.paths.borrow().is_empty());
}

#[test]
fn failed_retirement_preserves_history_and_does_not_block_available_storage() {
    struct FailedRetirer;
    impl WorkspaceRetirement for FailedRetirer {
        fn retire(&self, _: &WorktreeRetirementSpec) -> Result<RetirementResult, AllocationError> {
            Err(AllocationError::InvalidConfiguration)
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let policy = policy(&directory, 500);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store.create_case(&terminal_case(policy.revision)).unwrap();
    let probe = SequenceProbe {
        snapshots: RefCell::new(vec![WorkspaceStorageSnapshot {
            free_bytes: 1_000,
            distinct_filesystem: true,
        }]),
    };
    let cycle =
        reconcile_workspace_lifecycle_once_with(&mut store, &policy, 300, &probe, &FailedRetirer)
            .unwrap();
    assert!(cycle.ready);
    assert!(cycle.retired_case_key.is_none());
    assert!(cycle.cleanup_error.is_some());
    assert_eq!(store.status(300).unwrap().workspace_retirements, 0);
}
