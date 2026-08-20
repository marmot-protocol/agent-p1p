//! Controller-owned worktree and fresh worker execution boundaries.

#![forbid(unsafe_code)]

mod process;
mod provider;
mod worktree;

pub use process::{
    BoundedProcessRunner, ProcessError, ProcessOutput, ProcessRunner, ProcessSpec,
    sanitized_environment,
};
pub use provider::{CursorHealthProbe, HealthAssurance, ProviderHealth, ProviderProbeError};

pub use worktree::{
    AllocationError, AllocationResult, GitCommand, GitOutput, GitRunner, ProcessGitRunner,
    WorktreeAllocator, WorktreeSpec,
};
