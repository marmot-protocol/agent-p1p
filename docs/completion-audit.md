# Pip v2 completion audit

**Snapshot date:** 2026-08-20

**Decision:** The repository-local Rust control plane is feature-complete for
an inert, single-repository shadow deployment. It is not production-ready until
the external gates below are satisfied, and it is not ready for Phase 10
multi-repository expansion until the Phase 9 MDK canary is accepted.

This audit maps the canonical architecture and migration roadmap to executable
evidence. `Implemented locally` means source plus tests exist. It does not mean
the behavior ran with live service identities, credentials, providers, or an
authorized issue.

## Architecture coverage

| Target boundary | Status | Executable evidence |
|---|---|---|
| Generic repository/board/case identity | Implemented locally | Strict repository policy, numeric repository/actor validation, generic case identity, and no compiled canary issue |
| Deterministic Rust workflow | Implemented locally | `pip-core` states/events/effects and property/fixture tests |
| Authoritative durable ledger | Implemented locally | SQLite schema v5, immutable history, webhook deliveries, outbox, projections, attempts, migrations, backup, and crash injection |
| Signed webhook primary intake adapter | Implemented locally; ingress external | HMAC verification, delivery replay/conflict checks, exact repository/issue/actor re-read, and `webhook-intake` CLI |
| Bounded polling recovery | Implemented locally | Generic label discovery and live-evidence eligibility reconciliation |
| Planner before builder | Implemented locally | Typed planner contract, durable plan publication gate, and dispatch ordering |
| Assigned builder worktree and draft PR | Implemented locally | Controller-owned checkout/worktree/branch, credential-free builder, exact push, and stable draft-PR transaction |
| Exact-head CI and two independent reviews | Implemented locally | CI reconciliation, distinct review identities, role stamps, exact-head joins, and publication retries |
| Dynamic remediation and convergence | Implemented locally | State-driven redispatch rather than a fixed DAG; round, elapsed-time, repeated-finding, direct-attempt, and Hermes circuit-breaker bounds |
| Holistic final review | Implemented locally | Atomic final preflight plus full immutable evidence bundle |
| Shadow disposition | Implemented locally and selected for MDK | `READY` becomes `SHADOW_READY`; autonomous merge is unreachable under checked-in MDK policy |
| Guarded merge | Implemented locally, disabled for MDK | Separate ready-for-review, revalidation, expected-head merge, and restart-convergence transaction |
| Runtime isolation | Implemented locally | Separate control/worker identities, service-owned Hermes root, credential-free worker surfaces, and systemd contract tests |
| Release provenance and rollback | Implemented locally; trusted CI run external | Signed source-bound cohort, pinned actions/image, protected release workflow, installer verification, and passing disposable-systemd lifecycle |
| In-flight policy revision changes | Safe for the frozen canary; Phase 10 expansion | Policy revisions are immutable and mismatches fail closed; explicit restrictive overlays and nonrestrictive hot migration are deferred until after Phase 9 acceptance |
| Global provider/registry/reporting layer | Sequenced after canary | Shared ledger and global active-case limit exist; registry operations, health aggregation, dependency routing, and multi-board reporting remain Phase 10 |

## Roadmap disposition

| Phase | Disposition |
|---|---|
| 0 — architecture and safety boundaries | Complete in repository; documents reconciled by this audit |
| 1 — frozen reference behavior | Complete; retained Python fixtures remain parity inputs, not runtime authority |
| 2 — pure Rust core | Complete locally |
| 3 — ledger and outbox | Complete locally through schema v5 |
| 4 — read-only adapters | Complete locally; live service-identity probes remain Phase 8 evidence |
| 5 — projections and execution | Complete locally for Hermes and direct Cursor paths |
| 6 — controlled GitHub writes | Complete locally; live credential scope evidence remains external |
| 7 — packaging and lifecycle | Complete locally; protected signing environment and exact CI run remain external release gates |
| 8 — parity and non-dispatching live shadow | Local fixture/parity work complete; target-host Hermes/provider outage and recovery drill still required |
| 9 — one MDK shadow case | Not authorized and not run |
| 10 — controlled expansion | Intentionally not started before Phase 9 acceptance |
| Legacy retirement | Intentionally not started before cutover and soak approval |

## Verified local evidence

The following gates passed on 2026-08-20 after the schema v5 and runtime-bound
changes:

- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- `cargo test --workspace --locked`;
- `scripts/check-supply-chain-pins.sh` and its negative regression test; and
- `scripts/test-systemd-lifecycle.sh`, reporting clean install, reinstall,
  upgrade, injected rollback, and restart recovery with active timers disabled.

The workstation had Cursor `2025.09.18-7ae6800`, an authenticated personal
GitHub CLI, and Docker `29.6.2`, but no `hermes` executable. Personal GitHub
authentication is not service credential evidence.

## Inputs required before work can continue safely

These are external state or authority, not remaining opportunities for a local
implementation guess:

1. A target Linux/systemd host with a compatible Hermes installation and the
   service-owned Hermes/provider authentication paths.
2. A trusted webhook TLS ingress or relay, its GitHub webhook secret, and a
   byte-preserving mapping to `webhook-intake`.
3. Three distinct numeric GitHub actors and separately scoped controller,
   general-reviewer, and security/performance-reviewer credentials.
4. The actual required MDK CI contexts and confirmation that the canonical
   checkout/branch policy matches the target repository.
5. A protected `pip-release` GitHub environment with approved signing trust
   material, followed by an independently verified exact-head workflow run.
6. Explicit authorization to install/activate the inert services and label
   exactly one suitable MDK issue.
7. JG acceptance of the complete shadow result before Phase 10 or legacy
   retirement begins.

Until those inputs exist, the correct state is the checked-in one: intake
disabled, repository paused, dispatch disabled, shadow merge mode, autonomous
merge false, and no live Rust services enabled.
