# Control flow and source map

[Pip architecture](pip-architecture-plan.md) defines the target.
[Implementation status](implementation-status.md) distinguishes implemented
behavior from remaining work. This file maps that design onto the code.

## One authority

The Rust ledger owns decisions, accepted results and intended effects.
GitHub owns repository evidence. Hermes owns native execution and board state;
the Cursor adapter executes direct jobs. Neither executor decides workflow state.

An observation is validated before a ledger transaction records its decision
and outbox effects. External calls happen afterward and are reconciled using
stable ownership markers and exact identities. A timeout is not proof that an
external operation did not happen.

## Workflow

1. Signed webhook intake and bounded polling revalidate the live issue and
   trusted label actor, then create one case and planner intent.
2. Hermes executes the planner. Rust validates its result, retains the plan,
   publishes the plan comment idempotently and applies the typed outcome.
3. The builder receives its assigned workspace, branch, plan and exact model.
   It tests and commits locally; it has no GitHub publication credentials.
4. Rust validates the returned head, publishes only the assigned branch,
   verifies it and creates or updates one draft PR.
5. Independent GitHub CI evidence gates reviews for that exact head.
6. Required reviewer instances run independently. Their findings join by
   reviewer identity and head. Advisory/shadow results are observations only.
7. Changes requested enter bounded remediation. A new head requires new CI and
   required reviews. Prior approvals cannot approve a different commit.
8. Fresh GitHub preflight precedes final review. Its accepted readiness result
   becomes a human-held recommendation, not a merge operation.

The current shadow-policy terminal recommendation is named SHADOW_READY in the
ledger. Historical guarded-merge states remain readable, but current policy
validation rejects autonomous merge and no runtime merge writer exists.
An attempt marked RUNNING in the ledger can also be awaiting result ingestion;
use process/service evidence before concluding a worker is still executing.

## Implementation map

| Responsibility | Source |
|---|---|
| Pure states, events and policy decisions | crates/pip-core/src/state_machine.rs |
| Transactions, history, leases and projections | crates/pip-store/src/lib.rs |
| Job definitions and result joins | crates/pip-controller/src/scheduling.rs and results.rs |
| Process composition | crates/pip-control/src/cli.rs |
| Webhook verification and durable spool | crates/pip-control/src/webhook_http.rs, webhook_spool.rs and webhook_consumer.rs |
| Eligibility and revocation | crates/pip-control/src/intake.rs and authorization.rs |
| Hermes task projection and completion | crates/pip-control/src/dispatch.rs, results.rs and crates/pip-hermes/ |
| Direct inbox, attempt and result transport | crates/pip-control/src/direct_queue.rs and direct_worker.rs |
| Cursor execution and subprocess bounds | crates/pip-executor/src/cursor.rs and process.rs |
| Plan/PR/review publication and final checks | crates/pip-control/src/plans.rs, draft_pr.rs, ci.rs, reviews.rs and final_preflight.rs |
| Human-held disposition and takeover | crates/pip-control/src/disposition.rs and takeover.rs |
| Workspace allocation, publication and retention | crates/pip-executor/ and crates/pip-control/src/workspace_lifecycle.rs |
| Release verification and installation | crates/pip-control/src/release.rs, install.rs and scripts/install-rust-control-plane.sh |

## Recovery

Recover from saved ledger definitions, never task titles or model memory.
Keep completed results and uncertain external effects for reconciliation.
Do not reset a case, remove an authorization label or delete attempts merely
to bypass a failed infrastructure boundary.

Pause must eventually separate new work from safe completion collection; that
is a target requirement, not a claim that every current path supports it.
The status document records this and the remaining policy-upgrade and
failure-isolation gaps. For current deployment ordering and privileges, use
the [deployment runbook](runbooks/deployment.md).
