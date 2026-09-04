# Pip completion audit

**Snapshot date:** 2026-09-03

**Decision:** The deterministic Rust workflow and controller are installed on
Pirate as an inert, single-repository shadow deployment. Native reviewer-App
authentication is implemented. The built-in webhook ingress is still an
inert local integration: its isolated service/install boundary and
spool-to-ledger consumer now exist and pass the disposable lifecycle, but have
not been installed or exposed on Pirate. The system is not production-ready
until the external gates below are satisfied, and it is not ready for Phase 10
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
| Authoritative durable ledger | Implemented locally | SQLite schema v6, immutable history and workspace-retirement evidence, webhook deliveries, outbox, projections, attempts, migrations, backup, and crash injection |
| Signed webhook primary intake adapter | Implemented locally; Pirate deployment and TLS external | HMAC verification, delivery replay/conflict checks, exact repository/issue/actor re-read, isolated loopback receiver, atomic raw-body spool, controller-owned bounded consumption, commit-before-processed ordering, outage replay, tamper rejection, and systemd lifecycle tests |
| Bounded polling recovery | Implemented locally | Generic label discovery and live-evidence eligibility reconciliation |
| Planner before builder | Implemented locally | Typed planner contract, durable plan publication gate, and dispatch ordering |
| Assigned builder worktree and draft PR | Implemented locally | Controller-owned checkout/worktree/branch, credential-free builder, exact push, and stable draft-PR transaction |
| Bounded worktree storage | Installed, persistently mounted, and reboot-verified on Pirate | Dedicated-mount and free-space gates, 24-hour terminal retention, no-force clean retirement, running-attempt exclusion, one-per-cycle cleanup, immutable retirement evidence, and successful manual-remount and post-reboot systemd mount-unit probes |
| Pinned Hermes compatibility | Service-owned runtime bootstrapped inertly on Pirate | Exact `v2026.8.31` commit and installer hash, slug-based board identity, task/run JSON contract, typed workspaces, external-supervisor gateway flag, and successful exact-release service-root bootstrap |
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
| 3 — ledger and outbox | Complete locally through schema v6 |
| 4 — read-only adapters | Complete locally; live service-identity probes remain Phase 8 evidence |
| 5 — projections and execution | Complete locally for Hermes and direct Cursor paths |
| 6 — controlled GitHub writes | Complete locally; live credential scope evidence remains external |
| 7 — packaging and lifecycle | Inert local-bootstrap cohort installed on Pirate; protected signing environment and exact CI run remain external release gates |
| 8 — parity and non-dispatching live shadow | Inert host install, persistent workspace mount, canonical checkout, and service-owned Hermes bootstrap complete; provider capability plus outage and recovery drill still required |
| 9 — one MDK shadow case | Not authorized and not run |
| 10 — controlled expansion | Intentionally not started before Phase 9 acceptance |
| Legacy retirement | Vault runtime retired early on 2026-09-01 by explicit JG authorization; no Python rollback data retained, while repository source and curated parity fixtures remain |

## Verified local evidence

The following gates passed on 2026-09-03 after the schema v6 workspace-lifecycle
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

## Verified Pirate installation evidence

On 2026-09-03, corrected source commit
`ff15894be798d403ea28ac668f82a15dd30575fd` was built as a signed
local-bootstrap cohort and installed on Pirate under content-addressed release
ID `89786823409d5d18d612d2fb10412e04c240d1d34a958ada94b6a8b4ac1a91cc`.
It superseded the initial `9e2c9ee` cohort after a disposable live probe found
and corrected a Hermes v0.21 profile-output compatibility mismatch. The
installed binary, installer, public key, ownership boundary, empty schema-v6
ledger, persistent workspace bind mount, canonical checkout, disabled/inactive
execution units, and exact-release service-owned Hermes bootstrap were then
checked. See
[`evidence/2026-09-03-pirate-inert-install.md`](evidence/2026-09-03-pirate-inert-install.md).

This proves an inert installation, a mechanical unmount/remount cycle, and
correct mount recovery after a real reboot. It also proves creation and
effective-configuration verification of the isolated `pip-mdk` board and three
Hermes-native profiles. It does not prove a live provider API call,
GitHub-App credentials, webhook delivery, or a canary.

## Inputs required before work can continue safely

These are external state or authority, not remaining opportunities for a local
implementation guess:

1. A non-dispatching live provider capability probe and outage/recovery drill,
   plus confirmation that the separately supervised gateway uses the already
   bootstrapped service-owned Hermes root.
2. One controlled pending delivery through the installed Funnel, isolated
   receiver, and spool-to-ledger consumer. The live path has passed signed
   ping, replay, reachability, and empty-spool service probes without enabling
   the consumer timer.
3. Install policy revision 2 with the three verified numeric GitHub actors and
   complete an installed-key token-mint/read probe for each reviewer App. The
   narrowly scoped controller credential and both root-owned reviewer
   credential sets are installed.
4. Install policy revision 2's `Required CI` context and reverify it against the
   live active `Safe Master` rule sourced from GitHub Actions.
5. A protected `pip-release` GitHub environment with approved signing trust
   material, followed by an independently verified exact-head workflow run.
6. Explicit authorization to activate the inert services and label exactly one
   suitable MDK issue. Installation alone grants no activation authority.
7. JG acceptance of the complete shadow result before Phase 10 or legacy
   retirement begins.

Until those inputs exist, the correct state is the checked-in one: intake
disabled, repository paused, dispatch disabled, shadow merge mode, autonomous
merge false, public ingress enabled, and all execution timers and gateways
disabled.
