//! Controller-owned worktree and fresh worker execution boundaries.

#![forbid(unsafe_code)]

mod worktree;

pub use worktree::{
    AllocationError, AllocationResult, GitCommand, GitOutput, GitRunner, ProcessGitRunner,
    WorktreeAllocator, WorktreeSpec,
};
