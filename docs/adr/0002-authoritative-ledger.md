# ADR 0002: Use the Rust ledger as workflow authority

- **Status:** Accepted
- **Date:** 2026-08-20

## Context

The Python prototype contains a SQLite permanent-case state machine, but its
deployed build/review flow lives primarily in Hermes Kanban task metadata. The
offline fixture advances SQLite through the full workflow; the installed route
consumer does not. This creates two partial workflow representations and leaves
restart, replay, DAG upgrade, and audit behavior ambiguous.

Hermes is valuable as a persistent repository-scoped queue, worker dispatcher,
and operator-facing board for Hermes-native roles. Direct-provider roles cannot
be represented as unsupported Hermes providers and require a controller-owned
durable queue. Neither execution path should become the domain database for
Pip's exact-head evidence and transition rules.

## Decision

The Rust control-plane ledger is the sole authority for:

- case identity and current state;
- accepted policy and authorization revisions;
- immutable worker runs and external evidence;
- plan, build, remediation, review, and final-review rounds;
- findings, resolutions, and confirmations;
- branch, worktree, PR, head, and merge evidence;
- transition history, leases, and retry/escalation counters; and
- intended external effects in a transactional outbox.

Hermes Kanban is one projection and execution boundary. The controller creates
Hermes-native tasks from committed outbox intent and records direct-provider
jobs as leased outbox work. It observes results from both paths, validates them,
and commits accepted results before dispatching children. A direct job may be
mirrored to Hermes for operator visibility only if the mirror cannot be claimed
or executed by the Hermes dispatcher.

GitHub is authoritative for GitHub object state, but a fetched GitHub snapshot
does not alter workflow state until the controller validates and records it.

## Transaction boundary

For each transition, one SQLite transaction:

1. verifies expected case revision and prior state;
2. inserts normalized evidence and/or an immutable run;
3. appends the domain event;
4. updates the current case projection; and
5. inserts required outbox effects.

External effects happen after commit and are idempotently reconciled. Their
observed completion creates another event; it does not rewrite the prior one.

## Consequences

- A Hermes board can be rebuilt from ledger projections, while a direct job
  remains the original durable outbox intent; neither invents case state.
- Board drift is a discrepancy to reconcile, not a source of transitions.
- Every worker completion is accepted at most once.
- DAG/version upgrades become new projections over durable case state rather
  than title/body matching against old tasks.
- SQLite backup and migration become critical operational responsibilities.
- Workers must not receive filesystem write access to the ledger.
- The controller needs a deliberate outbox, lease, and reconciliation design.

## Rejected alternatives

### Hermes as the sole authority

This would reduce local storage but make exact evidence joins, schema migration,
transactional dispatch, loop accounting, and reconstruction dependent on
Hermes task representation and retention semantics.

### Dual authority with reconciliation

Treating SQLite and Hermes as peers preserves the current ambiguity. Conflict
resolution would itself need an authority and would create unsafe edge cases
after partial failure.

### GitHub comments as the ledger

GitHub is a durable human audit surface, not a transactional workflow database.
Comments can be edited or deleted and cannot atomically bind internal dispatch
intent with state transitions.
