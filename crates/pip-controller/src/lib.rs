//! Deterministic effect scheduling for the Pip control plane.

#![forbid(unsafe_code)]

mod scheduling;

pub use scheduling::{
    DispatchContext, DispatchError, ExecutionKind, RolePolicy, WorkflowDispatch, WorkflowPolicy,
    schedule_effect,
};
