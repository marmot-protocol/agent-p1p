# Python reference to Rust control-plane roadmap

The migration is incremental. The Python prototype remains stopped for new
production installation but available as a fixture source until Rust cutover is
proven. Each phase has an exit gate; elapsed effort is not an exit condition.

## Phase 0: Documentation and design freeze

Deliverables:

- Correct README and canonical target architecture.
- Accurate legacy Python canary inventory.
- Rust-language and authoritative-ledger ADRs.
- Explicit control flow, recovery, deployment, and canary contracts.
- Gap-to-workstream roadmap.

Exit gate:

- Documentation consistently separates target, legacy, and activation policy.
- No target behavior requires a compiled-in repository, issue, comment, actor,
  notification destination, PR, or DAG revision.
- JG accepts the architectural direction.

## Phase 1: Freeze and curate reference behavior

Deliverables:

- Resolve or explicitly supersede the three baseline CI failures.
- Export language-neutral valid/invalid contract fixtures.
- Export state/event transition tables and decision-reconciler fixtures.
- Record exact-head join, actor authorization, and replay adversarial fixtures.
- Classify every legacy test as `port`, `replace`, or `legacy-only` with reason.

Exit gate:

- The reference fixture cohort is immutable and green at an exact commit.
- Known legacy bugs are not accidentally enshrined as desired behavior.

## Phase 2: Rust workspace and pure core

Strict TDD order:

1. Create the Cargo workspace and locked toolchain policy.
2. Add domain IDs, policy revisions, states, events, runs, findings, and effects.
3. Port transition tests before implementing transitions.
4. Implement deterministic policy evaluation and exact-head join decisions.
5. Add property tests for terminal states, stale revisions, idempotency, and
   invalid evidence.

Exit gate:

- `pip-core` has no I/O dependencies.
- Every state/event pair is exhaustively handled.
- Clock, IDs, and external observations are explicit inputs.
- The curated transition fixture cohort passes.

## Phase 3: Contracts, ledger, and outbox

Deliverables:

- Rust worker/evidence contracts with stable serialized versions.
- SQLite schema and transactional migrations.
- Immutable evidence/run/event tables and current-case projection.
- Transactional outbox, leases, and idempotency constraints.
- Crash injection around every commit/effect boundary.
- Backup, forward migration, and supported rollback behavior.

Exit gate:

- Restart/replay cannot duplicate an accepted run or outbox effect.
- The ledger reconstructs the current case solely from committed history.
- Workers cannot write the ledger in the integration harness.

## Phase 4: Read-only external adapters

Deliverables:

- GitHub authenticated reads, webhook signature validation, pagination, and
  canonical numeric identity.
- Hermes capability probe, board/task/result reads, and projection comparison.
- Provider/model/auth health probes with bounded time/output.
- Normalized external evidence types consumed by `pip-core`.
- Bounded issue title/body/comment snapshots with numeric authors and body
  digests for final-review reconstruction.

Exit gate:

- Fixture and sandbox tests cover malformed, oversized, stale, reordered, and
  replayed external data.
- No read adapter can dispatch, push, comment, or merge.

## Phase 5: Projection and worker execution

**Implementation status:** Locally complete. Hermes-native roles use
controller-gated board projections. Direct Cursor roles use durable attempts
and a controller-owned immutable inbox/result bridge to a separate service
identity with no ledger access. Exact policy-bound checkouts and case worktrees
are reconciled before projection, and both paths converge through the same
bound result-ingestion contract.

Deliverables:

- Idempotent Hermes task projection from outbox entries.
- Profile/skill/model/task binding validation.
- Controller-owned worktree and branch allocation.
- Fresh Hermes and direct-provider executor adapters.
- Bounded process leases, timeouts, cleanup, and artifact retention.
- Planner, builder, parallel review, remediation, and final-review projections.
- Revision-bound, digest-addressed immutable evidence bundles on every worker
  projection, including the accepted GitHub preflight for final review.

Exit gate:

- Offline end-to-end cases survive restart at every phase.
- Dynamic remediation supports more than one round and respects policy bounds.
- A worker cannot release its own child task.
- Hermes-native and direct-provider jobs are routed to different executors
  without model substitution, and both converge into the same bound result
  ingestion path.
- Every dispatched workspace is allocated first and encoded using the selected
  executor's exact path contract.

## Phase 6: GitHub write boundary

**Implementation status:** Locally complete. The idempotent write primitives and
adversarial tests exist. Authorization removal and foreign-PR takeover are
durable, and final-review dispatch now waits for exact published GitHub
evidence. Distinct role identities now publish the joined reviews before
remediation or final preflight, planner results cannot release their typed
outcome until the controller publishes the immutable plan comment, and
human-held issue/PR disposition comments and local terminal effects are
consumed transactionally. Builder results are also held until the controller
creates or updates the stable case-owned draft PR. Builder tasks now receive an
exact worktree/branch assignment and no GitHub credential; the controller
publishes the accepted clean local head with exact force-with-lease and remote
verification before touching the PR. Publication is confined to a canonical
controller-root worktree and policy-bound push URL while repository-controlled
hooks, filesystem monitors, credential helpers, proxies, and HTTP headers are
disabled and TLS verification is forced. Guarded merge is implemented behind
explicit guarded/autonomous policy and is unreachable under MDK shadow policy.
The signed `pip-control` binary now acts as Git askpass and passes only the
systemd credential-file path to Git. Live credential identity and scope evidence
remains a separate activation gate.

Deliverables:

- Idempotent issue comments and provenance markers.
- Pip-owned branch push and draft-PR create/update transactions.
- Review/finding publication without reviewer push authority.
- Human takeover, authorization removal, and duplicate ownership handling.
- Guarded merge implementation present but disabled for MDK policy.

Exit gate:

- Replayed effects do not duplicate comments, branches, or PRs.
- Exact-head mutation races fail closed.
- Role credentials are no broader than documented policy permits.

## Phase 7: Packaging and lifecycle

**Implementation status:** The signed cohort, protected manual CI release
workflow, exact action/container pins, installer, isolated control and
direct-worker identities, shadow unit, inert active-runtime templates, and
disposable lifecycle are complete locally.
Fresh install, reinstall, upgrade, rollback, and restart all preserve disabled
active and shadow timers. The protected signing environment is not provisioned,
so the workflow has not produced an authorized release. CI and live-host
evidence are separate gates.

Deliverables:

- Reproducible release cohort and signed source-bound manifest.
- Minimal root installer/upgrader.
- Hardened units, identities, credentials, and filesystem boundaries.
- Disposable-systemd clean install, reinstall, upgrade, rollback, and recovery
  harness in CI.
- Operator status and evidence commands.

Exit gate:

- Every installer mutation stage has a tested rollback.
- The installed binary/resources match the reviewed manifest.
- Intake/dispatch remain paused after a fresh install unless policy explicitly
  says otherwise.

## Phase 8: Parity and non-dispatching live shadow

**Implementation status:** Frozen transition and worker-contract parity is
exhaustive, repeated live GitHub reconciliation is stable with zero mutations,
and local adapter tests cover Hermes outage/recovery plus direct and Hermes
circuit breakers. Pirate now has the pinned Hermes installation, service-owned
root, canonical MDK checkout, and successful inert runtime bootstrap.
Separately supervised gateway observation and live provider capability plus
outage/recovery evidence remain open exit-gate items.

Deliverables:

- Rust evaluation of frozen Python fixtures.
- Recorded live GitHub/Hermes evidence ingested read-only.
- Decision/discrepancy comparison reports.
- Recovery and provider-outage drills.

Exit gate:

- Every intentional parity difference is documented and approved.
- Repeated reconciliation is stable and token-free.
- No Rust shadow operation creates or releases a task.

## Phase 9: One generic MDK shadow case

**Implementation status:** No live case has started. The generic Rust path,
signed-webhook adapter, polling recovery, runtime templates, and operational
bounds exist locally. Pirate now exposes the isolated loopback ingress through
Tailscale Funnel, and the installed durable spool consumer has passed an
empty-spool production-credential cycle while its timer remains disabled. A
controlled pending-delivery probe and the other host-specific gaps listed in
[`implementation-status.md`](implementation-status.md) must close before
activation, which remains unauthorized.

Deliverables:

- Reviewed exact release installed with MDK policy.
- Legacy/new intake ownership boundary verified.
- Exactly one issue deliberately tagged `pip-ok`.
- Full plan, draft PR, independent reviews, dynamic remediation if needed, final
  review, and human-held shadow disposition.
- Evidence comparison with JG.

Exit gate:

- One complete case is reconstructable after service restart.
- No duplicate issue, task, branch, PR, review, notification, or merge exists.
- Exact-head and model gates held throughout.
- JG accepts the shadow result quality and operational behavior.

## Phase 10: Controlled expansion

**Implementation status:** Sequenced after Phase 9 by design. The shared ledger
already enforces global active-case capacity across installed repository
policies, but provider health aggregation, repository registry operations,
cross-repository dependency routing, and multi-board reporting are not claimed
complete before the single-repository canary is accepted.

Phase 10 also owns explicit restrictive policy overlays and safe migration of
in-flight cases across nonrestrictive policy revisions. Phase 9 instead freezes
the installed policy revision for the duration of its single case and fails
closed on any mismatch.

Expand one dimension at a time:

1. More MDK issues with bounded concurrency.
2. Provider/global health and reporting under real load.
3. A second repository and board using the same engine.
4. Cross-repository dependency cases.
5. Guarded merge only for a separately approved repository/policy.

MDK autonomous merge remains disabled until JG explicitly changes it. Passing
the MDK shadow canary does not itself authorize merge or another repository.

## Legacy retirement

The original plan placed retirement after Rust cutover and soak. JG explicitly
authorized early retirement on 2026-09-01 and waived retention of the deployed
Python data. The `vault` services, timers, wheels, credentials, database, route,
`pip-mdk` board, and Pip-owned profile skill links were deleted. The
`pip-control` system identity was retained because it already satisfies the
Rust installer contract. There is no Python runtime rollback path.

The remaining source-retirement obligations are:

- retain the exact Python source and reference fixtures;
- keep Python excluded from the target build and release;
- use only curated fixtures as parity evidence; and
- remove the Python source only after the Rust canary no longer needs it as a
  migration reference and JG explicitly authorizes that repository change.
