//! Deterministic effect scheduling for the Pip control plane.

#![forbid(unsafe_code)]

mod ledger;
mod results;
mod scheduling;

pub use ledger::{ControllerError, LedgerController, WorkflowCommand};
pub use results::{
    IngestError, IngestResult, ingest_worker_result, ingest_worker_result_with_policy,
};

pub use scheduling::{
    DirectTaskSpec, DispatchContext, DispatchError, ExecutionKind, RolePolicy, WorkflowDispatch,
    WorkflowPolicy, review_snapshot, schedule_claimed_dispatch, schedule_effect,
};
