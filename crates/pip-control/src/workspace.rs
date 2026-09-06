//! Policy-bound canonical checkout reconciliation and case worktree allocation.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;
use std::time::Duration;

use pip_contracts::WorkerResult;
use pip_core::{CaseId, GitSha, IssueNumber, RepositoryId, WorkflowVersion};
use pip_executor::{
    AllocationError, BoundedProcessRunner, CheckoutError, CheckoutReconciler, IsolatedWorkspace,
    ProcessGitRunner, WorktreeSpec, sanitized_environment,
};
use pip_store::{ClaimedEffect, Store, StoreError, StoredCase};

use crate::RepositoryPolicy;

#[derive(Debug)]
pub enum WorkspaceError {
    InvalidCase,
    MissingPlanBase,
    Checkout(CheckoutError),
    Allocation(AllocationError),
    Store(StoreError),
    Lifecycle(crate::WorkspaceLifecycleError),
    MalformedRun(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCase => formatter.write_str("dispatch case cannot bind a worktree"),
            Self::MissingPlanBase => {
                formatter.write_str("dispatch requires one active planned-base commit")
            }
            Self::Checkout(error) => error.fmt(formatter),
            Self::Allocation(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Lifecycle(error) => error.fmt(formatter),
            Self::MalformedRun(error) => write!(formatter, "malformed planner run: {error}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<CheckoutError> for WorkspaceError {
    fn from(error: CheckoutError) -> Self {
        Self::Checkout(error)
    }
}

impl From<AllocationError> for WorkspaceError {
    fn from(error: AllocationError) -> Self {
        Self::Allocation(error)
    }
}

impl From<StoreError> for WorkspaceError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<crate::WorkspaceLifecycleError> for WorkspaceError {
    fn from(error: crate::WorkspaceLifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}

pub trait WorkspacePreparer {
    fn prepare_dispatch_storage(
        &self,
        _policy: &RepositoryPolicy,
        _store: &Store,
        _dispatches: &[pip_controller::WorkflowDispatch],
    ) -> Result<(), WorkspaceError> {
        Ok(())
    }
    fn prepare(
        &self,
        policy: &RepositoryPolicy,
        claimed: &ClaimedEffect,
        case: &StoredCase,
        store: &Store,
    ) -> Result<(), WorkspaceError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GitWorkspacePreparer;

impl WorkspacePreparer for GitWorkspacePreparer {
    fn prepare_dispatch_storage(
        &self,
        policy: &RepositoryPolicy,
        store: &Store,
        dispatches: &[pip_controller::WorkflowDispatch],
    ) -> Result<(), WorkspaceError> {
        if policy.hermes_scratch_root.is_some() {
            for dispatch in dispatches
                .iter()
                .filter(|dispatch| dispatch.execution() == pip_controller::ExecutionKind::Hermes)
            {
                let task = dispatch
                    .hermes_task()
                    .map_err(|error| WorkspaceError::MalformedRun(error.to_string()))?;
                crate::prepare_hermes_scratch(policy, store, &task.body)
                    .map_err(WorkspaceError::MalformedRun)?;
            }
        }
        Ok(())
    }
    fn prepare(
        &self,
        policy: &RepositoryPolicy,
        claimed: &ClaimedEffect,
        case: &StoredCase,
        store: &Store,
    ) -> Result<(), WorkspaceError> {
        crate::workspace_lifecycle::ensure_workspace_storage_ready(policy, store.path())?;
        if claimed.case_key != case.case_key || claimed.state_revision != case.state_revision {
            return Err(WorkspaceError::InvalidCase);
        }
        let case_id = case_id(case)?;
        let expected_remote = format!(
            "https://github.com/{}/{}.git",
            policy.repository.owner, policy.repository.name
        );
        let reconciler = CheckoutReconciler::new(
            BoundedProcessRunner,
            "git",
            sanitized_environment(),
            Duration::from_secs(60),
            4 * 1024 * 1024,
        )?;
        let default_head = reconciler.fetch_default_head(
            &policy.checkout,
            &expected_remote,
            &policy.repository.default_branch,
        )?;
        let expected_head = expected_worktree_head(case, store, default_head)?;
        let spec = WorktreeSpec::new(
            &policy.checkout,
            &policy.workspace,
            &policy.branch_prefix,
            case_id,
            expected_head,
        )?;
        let allocator = IsolatedWorkspace::new(
            ProcessGitRunner,
            "git",
            Duration::from_secs(60),
            4 * 1024 * 1024,
        )?;
        allocator.allocate(&spec, &expected_remote)?;
        reconciler.verify_worktree(spec.path(), spec.branch(), expected_head)?;
        Ok(())
    }
}

fn expected_worktree_head(
    case: &StoredCase,
    store: &Store,
    default_head: GitSha,
) -> Result<GitSha, WorkspaceError> {
    if let Some(head) = &case.head_sha {
        return GitSha::from_str(head).map_err(|_| WorkspaceError::InvalidCase);
    }
    if case.plan_version == 0 {
        return Ok(default_head);
    }
    let mut planned = Vec::new();
    for run in store.runs_for_case(&case.case_key)? {
        let result: WorkerResult = serde_json::from_value(run.payload)
            .map_err(|error| WorkspaceError::MalformedRun(error.to_string()))?;
        if let WorkerResult::Planner(result) = result
            && result.plan_version == case.plan_version
        {
            planned.push(result.planned_base_sha);
        }
    }
    match planned.as_slice() {
        [head] => GitSha::from_str(head).map_err(|_| WorkspaceError::MissingPlanBase),
        _ => Err(WorkspaceError::MissingPlanBase),
    }
}

fn case_id(case: &StoredCase) -> Result<CaseId, WorkspaceError> {
    let id = CaseId::new(
        RepositoryId::new(NonZeroU64::new(case.repository_id).ok_or(WorkspaceError::InvalidCase)?),
        IssueNumber::new(NonZeroU64::new(case.issue_number).ok_or(WorkspaceError::InvalidCase)?),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(WorkspaceError::InvalidCase)?,
        ),
    );
    if id.to_string() != case.case_key {
        return Err(WorkspaceError::InvalidCase);
    }
    Ok(id)
}
