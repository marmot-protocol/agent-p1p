//! Deterministic effect scheduling for the Pip control plane.

#![forbid(unsafe_code)]

mod ledger;
mod scheduling;

pub use ledger::{ControllerError, LedgerController, WorkflowCommand};

pub use scheduling::{
    DispatchContext, DispatchError, ExecutionKind, RolePolicy, WorkflowDispatch, WorkflowPolicy,
    schedule_claimed_dispatch, schedule_effect,
};
