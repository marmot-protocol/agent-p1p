//! Pure domain and policy core for the Pip control plane.
//!
//! This crate deliberately contains no filesystem, network, clock, process,
//! database, GitHub, Hermes, or model-provider adapters.

#![forbid(unsafe_code)]

mod domain;
mod state_machine;

pub use domain::{
    CaseId, FindingId, GitSha, IdentifierError, IssueNumber, PlanVersion, PolicyRevision,
    RepositoryId, RepositorySlug, RunId, StateRevision, WorkflowVersion,
};
pub use state_machine::{
    CaseState, Effect, Event, MergeMode, ParseDomainValueError, TransitionContext,
    TransitionDecision, TransitionError, transition,
};
