# Pip completion audit

**Snapshot date:** 2026-09-04

**Decision:** The deterministic Rust workflow and controller are installed on
Pirate as an inert, single-repository shadow deployment. Native reviewer-App
authentication is implemented. The isolated webhook ingress is installed and
publicly reachable through the authenticated forwarding boundary; its consumer
and every execution path remain disabled. Policy revision 3 and its
policy-defined reviewer set are installed, but the system is not
production-ready until the external gates below are satisfied, and it is not
ready for Phase 10 multi-repository expansion until the Phase 9 MDK canary is
accepted.

This audit maps the canonical architecture and migration roadmap to executable
evidence. `Implemented locally` means source plus tests exist. It does not mean
the behavior ran with live service identities, credentials, providers, or an
authorized issue.

## Architecture coverage

| Target boundary | Status | Executable evidence |
|---|---|---|
| Generic repository/board/case identity | Implemented locally | Strict repository policy, numeric repository/actor validation, generic case identity, and no compiled canary issue |
| Deterministic Rust workflow | Implemented locally | `pip-core` states/events/effects and property/fixture tests |
| Authoritative durable ledger | Implemented locally | SQLite schema v7, immutable required-run history, detached review observations, workspace-retirement evidence, webhook deliveries, outbox, projections, attempts, migrations, backup, and crash injection |
| Signed webhook primary intake adapter | Installed on Pirate; intake policy and consumer timer disabled | HMAC verification, delivery replay/conflict checks, exact repository/issue/actor re-read, isolated loopback receiver, atomic raw-body spool, controller-owned bounded consumption, commit-before-processed ordering, outage replay, tamper rejection, systemd lifecycle tests, and live signed delivery/replay evidence |
| Bounded polling recovery | Implemented locally | Generic label discovery and live-evidence eligibility reconciliation |
| Planner before builder | Implemented locally | Typed planner contract, durable plan publication gate, and dispatch ordering |
| Assigned builder worktree and draft PR | Implemented locally | Controller-owned checkout/worktree/branch, credential-free builder, exact push, and stable draft-PR transaction |
| Bounded worktree storage | Installed, persistently mounted, and reboot-verified on Pirate | Dedicated-mount and free-space gates, 24-hour terminal retention, no-force clean retirement, running-attempt exclusion, one-per-cycle cleanup, immutable retirement evidence, and successful manual-remount and post-reboot systemd mount-unit probes |
| Pinned Hermes compatibility | Service-owned runtime bootstrapped inertly on Pirate | Exact `v2026.8.31` commit and installer hash, slug-based board identity, task/run JSON contract, typed workspaces, external-supervisor gateway flag, and successful exact-release service-root bootstrap |
| Exact-head CI and policy-defined independent reviews | Implemented locally | CI reconciliation, stable reviewer-instance identities, required/advisory/shadow modes, all-required exact-head joins, two semantic-lane review aggregates, and publication retries |
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
| 3 — ledger and outbox | Complete locally through schema v7 |
| 4 — read-only adapters | Complete locally with live controller and reviewer-App read probes on Pirate |
| 5 — projections and execution | Complete locally for Hermes and direct Cursor paths |
| 6 — controlled GitHub writes | Complete locally; live read scopes are proven and authorized write behavior remains reserved for the canary |
| 7 — packaging and lifecycle | Exact source `e4cbd33` installed inertly on Pirate with live schema migration; protected signing environment and exact CI run remain external release gates |
| 8 — parity and non-dispatching live shadow | Complete: inert host install, storage, checkout, runtime bootstrap, live GitHub reconciliation, configured-model capability probes, process-scoped provider outage/recovery, and empty-board supervised gateway observation passed |
| 9 — one MDK shadow case | Not authorized and not run |
| 10 — controlled expansion | Intentionally not started before Phase 9 acceptance |
| Legacy retirement | Vault runtime retired early on 2026-09-01 by explicit JG authorization; no Python rollback data retained, while repository source and curated parity fixtures remain |

## Verified local evidence

The following gates passed on 2026-09-04 for exact source `e4cbd33` after the
schema 6-to-7 installation regression was added and fixed:

- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `cargo test --workspace --locked`;
- `scripts/check-supply-chain-pins.sh` and its negative regression test; and
- `scripts/test-systemd-lifecycle.sh`, reporting clean install, reinstall,
  upgrade, injected rollback, and restart recovery with active timers disabled.

## Verified Pirate installation evidence

On 2026-09-04, source commit
`e4cbd333ede4197c31349b9e7259ee670311ed1e` was built as a signed cohort and
installed on Pirate under content-addressed release ID
`99099430157256bfb922226afe698c2a650fdeb5e3bcff0699e29f40ac68a78b`.
The live upgrade migrated the schema-6 ledger to schema 7 and preserved all
eight webhook-delivery records. Policy revision 3, the exact installed source,
ownership boundaries, service states, and Hermes profile reconciliation were
verified. Real read-only calls under `pip-worker` succeeded for the configured
Grok, Kimi, and Opus models, and the manual GitHub shadow reconciler found zero
candidates and made zero mutations. A forced loopback connection refusal then
recovered through a fresh exact Kimi request without changing the production
ledger. The disabled Pip gateway was also observed briefly under its exact
service identity and service-owned Hermes root while its board remained empty.
See
[`evidence/2026-09-04-pirate-policy-driven-reviewers.md`](evidence/2026-09-04-pirate-policy-driven-reviewers.md).

## Inputs required before work can continue safely

These are external state or authority, not remaining opportunities for a local
implementation guess:

1. An independently verified exact-head run of the configured protected
   `pip-release` deployment workflow.
2. Explicit authorization to activate the inert services and label exactly one
   suitable MDK issue. Installation alone grants no activation authority.
3. JG acceptance of the complete shadow result before Phase 10 or legacy
   retirement begins.

Installed policy revision 3, both reviewer App token-mint/read probes, and the
live `Safe Master` / GitHub Actions `Required CI` match are recorded in
[`evidence/2026-09-04-pirate-reviewer-apps.md`](evidence/2026-09-04-pirate-reviewer-apps.md).
The completed configured-label add/live-reread/remove proof is recorded in
[`evidence/2026-09-04-pirate-webhook-consumer.md`](evidence/2026-09-04-pirate-webhook-consumer.md).

Until those inputs exist, the correct state is the checked-in one: intake
disabled, repository paused, dispatch disabled, shadow merge mode, autonomous
merge false, public ingress enabled, and all execution timers and gateways
disabled.
