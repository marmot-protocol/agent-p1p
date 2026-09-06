//! Controller-owned worktree and fresh worker execution boundaries.

#![forbid(unsafe_code)]

mod checkout;
mod cursor;
mod isolated_workspace;
mod process;
mod provider;
mod publication;

pub use checkout::{CheckoutError, CheckoutReconciler};
pub use cursor::{CursorExecutionError, CursorExecutor, CursorTask};
pub use isolated_workspace::{IsolatedWorkspace, workspace_git_environment};
mod worktree;

pub use process::{
    BoundedProcessRunner, ProcessError, ProcessOutput, ProcessRunner, ProcessSpec,
    sanitized_environment,
};
pub use provider::{CursorHealthProbe, HealthAssurance, ProviderHealth, ProviderProbeError};
pub use publication::{GitPublicationSpec, GitPublisher, PublicationError, PublicationResult};

pub use worktree::{
    AllocationError, AllocationResult, GitCommand, GitOutput, GitRunner, ProcessGitRunner,
    RetirementResult, WorktreeAllocator, WorktreeRetirementSpec, WorktreeRetirer, WorktreeSpec,
};
