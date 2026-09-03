//! Fail-closed workspace storage checks and terminal worktree retirement.

use std::fmt;
use std::fs;
use std::num::{NonZeroU32, NonZeroU64};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use pip_core::{CaseId, IssueNumber, RepositoryId, WorkflowVersion};
use pip_executor::{
    AllocationError, ProcessGitRunner, RetirementResult, WorktreeRetirementSpec, WorktreeRetirer,
};
use pip_store::{Store, StoreError, WorkspaceRetirementInput, WorkspaceRetirementOutcome};
use serde::Serialize;

use crate::RepositoryPolicy;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceStorageSnapshot {
    pub free_bytes: u64,
    pub distinct_filesystem: bool,
}

pub trait WorkspaceStorageProbe {
    fn inspect(
        &self,
        workspace: &Path,
        ledger: &Path,
    ) -> Result<WorkspaceStorageSnapshot, WorkspaceLifecycleError>;
}

pub trait WorkspaceRetirement {
    fn retire(&self, spec: &WorktreeRetirementSpec) -> Result<RetirementResult, AllocationError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemWorkspaceStorageProbe;

impl WorkspaceStorageProbe for SystemWorkspaceStorageProbe {
    fn inspect(
        &self,
        workspace: &Path,
        ledger: &Path,
    ) -> Result<WorkspaceStorageSnapshot, WorkspaceLifecycleError> {
        let workspace_metadata = fs::symlink_metadata(workspace)
            .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
        if workspace_metadata.file_type().is_symlink() || !workspace_metadata.is_dir() {
            return Err(WorkspaceLifecycleError::InvalidWorkspace);
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
        let ledger = ledger
            .canonicalize()
            .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
        let workspace_metadata = fs::metadata(&workspace)
            .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
        let ledger_metadata = fs::metadata(ledger)
            .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
        let statistics = rustix::fs::statvfs(&workspace)
            .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
        let free_bytes = statistics
            .f_bavail
            .checked_mul(statistics.f_frsize)
            .ok_or_else(|| WorkspaceLifecycleError::Storage("free byte count overflow".into()))?;
        Ok(WorkspaceStorageSnapshot {
            free_bytes,
            distinct_filesystem: workspace_metadata.dev() != ledger_metadata.dev(),
        })
    }
}

pub struct GitWorkspaceRetirement {
    retirer: WorktreeRetirer<ProcessGitRunner>,
}

impl GitWorkspaceRetirement {
    pub fn new() -> Result<Self, WorkspaceLifecycleError> {
        Ok(Self {
            retirer: WorktreeRetirer::new(
                ProcessGitRunner,
                "git",
                Duration::from_secs(60),
                4 * 1024 * 1024,
            )?,
        })
    }
}

impl WorkspaceRetirement for GitWorkspaceRetirement {
    fn retire(&self, spec: &WorktreeRetirementSpec) -> Result<RetirementResult, AllocationError> {
        self.retirer.retire(spec)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceLifecycleCycle {
    pub ready: bool,
    pub free_bytes: u64,
    pub minimum_free_bytes: u64,
    pub retired_case_key: Option<String>,
    pub retired_worktree_path: Option<String>,
}

#[derive(Debug)]
pub enum WorkspaceLifecycleError {
    InvalidCase,
    InvalidWorkspace,
    SharedFilesystem,
    InsufficientCapacity { available: u64, required: u64 },
    Storage(String),
    Allocation(AllocationError),
    Store(StoreError),
}

impl fmt::Display for WorkspaceLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCase => {
                formatter.write_str("workspace candidate has an invalid case identity")
            }
            Self::InvalidWorkspace => {
                formatter.write_str("workspace root must be a real directory")
            }
            Self::SharedFilesystem => {
                formatter.write_str("workspace root is not on its required dedicated filesystem")
            }
            Self::InsufficientCapacity {
                available,
                required,
            } => write!(
                formatter,
                "workspace filesystem has {available} free bytes; {required} are required"
            ),
            Self::Storage(error) => write!(formatter, "workspace storage check failed: {error}"),
            Self::Allocation(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for WorkspaceLifecycleError {}

impl From<AllocationError> for WorkspaceLifecycleError {
    fn from(error: AllocationError) -> Self {
        Self::Allocation(error)
    }
}

impl From<StoreError> for WorkspaceLifecycleError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub fn reconcile_workspace_lifecycle_once(
    store: &mut Store,
    policy: &RepositoryPolicy,
    now: u64,
) -> Result<WorkspaceLifecycleCycle, WorkspaceLifecycleError> {
    reconcile_workspace_lifecycle_once_with(
        store,
        policy,
        now,
        &SystemWorkspaceStorageProbe,
        &GitWorkspaceRetirement::new()?,
    )
}

pub fn reconcile_workspace_lifecycle_once_with<P: WorkspaceStorageProbe, R: WorkspaceRetirement>(
    store: &mut Store,
    policy: &RepositoryPolicy,
    now: u64,
    probe: &P,
    retirer: &R,
) -> Result<WorkspaceLifecycleCycle, WorkspaceLifecycleError> {
    let initial = probe.inspect(Path::new(&policy.workspace), store.path())?;
    ensure_filesystem(policy, initial)?;
    let cutoff = now.saturating_sub(policy.workspace_storage.terminal_retention_seconds);
    let candidate = store
        .terminal_workspace_candidates(policy.repository.id, cutoff, 1)?
        .into_iter()
        .next();
    let mut retired_case_key = None;
    let mut retired_worktree_path = None;
    let snapshot = if let Some(candidate) = candidate {
        let case_id = CaseId::new(
            RepositoryId::new(
                NonZeroU64::new(candidate.repository_id)
                    .ok_or(WorkspaceLifecycleError::InvalidCase)?,
            ),
            IssueNumber::new(
                NonZeroU64::new(candidate.issue_number)
                    .ok_or(WorkspaceLifecycleError::InvalidCase)?,
            ),
            WorkflowVersion::new(
                NonZeroU32::new(candidate.workflow_version)
                    .ok_or(WorkspaceLifecycleError::InvalidCase)?,
            ),
        );
        if case_id.to_string() != candidate.case_key {
            return Err(WorkspaceLifecycleError::InvalidCase);
        }
        let spec = WorktreeRetirementSpec::new(
            &policy.checkout,
            &policy.workspace,
            &policy.branch_prefix,
            case_id,
        )?;
        let outcome = retirer.retire(&spec)?;
        let path = spec.path().to_string_lossy().into_owned();
        store.record_workspace_retirement(&WorkspaceRetirementInput {
            case_key: candidate.case_key.clone(),
            state_revision: candidate.state_revision,
            worktree_path: path.clone(),
            outcome: match outcome {
                RetirementResult::Retired => WorkspaceRetirementOutcome::Retired,
                RetirementResult::Absent => WorkspaceRetirementOutcome::Absent,
            },
            retired_at: now,
        })?;
        retired_case_key = Some(candidate.case_key);
        retired_worktree_path = Some(path);
        let refreshed = probe.inspect(Path::new(&policy.workspace), store.path())?;
        ensure_filesystem(policy, refreshed)?;
        refreshed
    } else {
        initial
    };
    Ok(WorkspaceLifecycleCycle {
        ready: snapshot.free_bytes >= policy.workspace_storage.minimum_free_bytes,
        free_bytes: snapshot.free_bytes,
        minimum_free_bytes: policy.workspace_storage.minimum_free_bytes,
        retired_case_key,
        retired_worktree_path,
    })
}

fn ensure_filesystem(
    policy: &RepositoryPolicy,
    snapshot: WorkspaceStorageSnapshot,
) -> Result<(), WorkspaceLifecycleError> {
    if policy.workspace_storage.require_distinct_filesystem && !snapshot.distinct_filesystem {
        Err(WorkspaceLifecycleError::SharedFilesystem)
    } else {
        Ok(())
    }
}

pub(crate) fn ensure_workspace_storage_ready(
    policy: &RepositoryPolicy,
    ledger: &Path,
) -> Result<WorkspaceStorageSnapshot, WorkspaceLifecycleError> {
    let snapshot = SystemWorkspaceStorageProbe.inspect(Path::new(&policy.workspace), ledger)?;
    ensure_filesystem(policy, snapshot)?;
    ensure_workspace_capacity(policy, snapshot)?;
    Ok(snapshot)
}

pub(crate) fn ensure_direct_workspace_capacity(
    policy: &RepositoryPolicy,
) -> Result<(), WorkspaceLifecycleError> {
    let workspace = Path::new(&policy.workspace);
    let metadata = fs::symlink_metadata(workspace)
        .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WorkspaceLifecycleError::InvalidWorkspace);
    }
    let statistics = rustix::fs::statvfs(workspace)
        .map_err(|error| WorkspaceLifecycleError::Storage(error.to_string()))?;
    let free_bytes = statistics
        .f_bavail
        .checked_mul(statistics.f_frsize)
        .ok_or_else(|| WorkspaceLifecycleError::Storage("free byte count overflow".into()))?;
    ensure_workspace_capacity(
        policy,
        WorkspaceStorageSnapshot {
            free_bytes,
            distinct_filesystem: true,
        },
    )?;
    Ok(())
}

fn ensure_workspace_capacity(
    policy: &RepositoryPolicy,
    snapshot: WorkspaceStorageSnapshot,
) -> Result<(), WorkspaceLifecycleError> {
    if snapshot.free_bytes < policy.workspace_storage.minimum_free_bytes {
        Err(WorkspaceLifecycleError::InsufficientCapacity {
            available: snapshot.free_bytes,
            required: policy.workspace_storage.minimum_free_bytes,
        })
    } else {
        Ok(())
    }
}
