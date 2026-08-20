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

Exit gate:

- Fixture and sandbox tests cover malformed, oversized, stale, reordered, and
  replayed external data.
- No read adapter can dispatch, push, comment, or merge.

## Phase 5: Projection and worker execution

Deliverables:

- Idempotent Hermes task projection from outbox entries.
- Profile/skill/model/task binding validation.
- Controller-owned worktree and branch allocation.
- Fresh Hermes and direct-provider executor adapters.
- Bounded process leases, timeouts, cleanup, and artifact retention.
- Planner, builder, parallel review, remediation, and final-review projections.

Exit gate:

- Offline end-to-end cases survive restart at every phase.
- Dynamic remediation supports more than one round and respects policy bounds.
- A worker cannot release its own child task.

## Phase 6: GitHub write boundary

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

**Implementation status:** Complete locally; CI and live-host evidence remain
separate gates.

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

Expand one dimension at a time:

1. More MDK issues with bounded concurrency.
2. Provider/global health and reporting under real load.
3. A second repository and board using the same engine.
4. Cross-repository dependency cases.
5. Guarded merge only for a separately approved repository/policy.

MDK autonomous merge remains disabled until JG explicitly changes it. Passing
the MDK shadow canary does not itself authorize merge or another repository.

## Legacy retirement

After Rust cutover and the approved soak:

- stop and disable legacy services;
- snapshot the Python database, units, policy, installed wheel/source
  descriptors, profiles, and board state;
- retain the exact Python source and reference fixtures;
- remove credentials from legacy service access;
- document the supported rollback window; and
- delete no historical artifacts or infrastructure without explicit approval.
