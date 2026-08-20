//! Pure domain and policy core for the Pip control plane.
//!
//! This crate deliberately contains no filesystem, network, clock, process,
//! database, GitHub, Hermes, or model-provider adapters.

#![forbid(unsafe_code)]

mod domain;
mod exact_head;
mod policy;
mod revision;
mod state_machine;

pub use domain::{
    ActorId, CaseId, EventId, FindingId, GitSha, IdentifierError, IssueNumber, ObservedAt,
    PlanVersion, PolicyRevision, PullRequestNumber, RepositoryId, RepositorySlug, RunId,
    StateRevision, WorkflowVersion,
};
pub use exact_head::{
    BuilderEvidence, CiEvidence, ExactHeadDecision, ExactHeadObservation, FindingEvidence,
    HeadBinding, JoinBlocker, ReviewEvidence, ReviewRole, evaluate_exact_head,
};
pub use policy::{IntakeBlocker, IntakeDecision, IntakePolicy, IssueObservation, evaluate_intake};
pub use revision::{
    CaseCommand, CaseCommandDecision, CasePolicy, CaseSnapshot, CommandError, evaluate_case_command,
};
pub use state_machine::{
    CaseState, Effect, Event, MergeMode, ParseDomainValueError, TransitionContext,
    TransitionDecision, TransitionError, transition,
};
