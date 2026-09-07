# Pip completion audit

Snapshot: 2026-09-07. **Not complete.** The approved target is
[pip-architecture-plan.md](pip-architecture-plan.md); detailed dated observations
are in [implementation-status.md](implementation-status.md). The earlier
phase-based audit is retained in Git history, not a second target.

## Live issue gate

MDK #993 has accepted plan 1 and draft
[PR #1726](https://github.com/marmot-protocol/mdk/pull/1726). The initial build
passed required CI and received independent App reviews: general requested
changes; security/performance approved. Builder attempt 14 produced an accepted
remediation at `625bb4299187139461a64fd6eb5337ea35804261`.

Signed Pip `8d3dde498fe60da1b4be9b5f2728610e6cdedcdb` is installed on Pirate.
Its normal publication action pushed that exact commit and updated the existing
PR, preserving accepted history. The case is `WAITING_CI`, revision 22. Fresh
GitHub CI run `34104104224` is in progress. The old head's CI and reviews do not
prove readiness of this new head. The PR is still draft; no merge occurred.

Required remaining live evidence:

1. Successful required CI on the published remediation head.
2. All required independent re-reviews on that same head, including origin
   confirmation of the blocking finding's resolution. Actual Cursor verification
   commands and the native review scratch layout must work in their sandboxes.
3. Current authorization, ownership, clean mergeability, and no unresolved
   blocking reviews or threads in the final preflight.
4. A fresh holistic final review and a published human-held readiness result.
5. Any further remediation must repeat exact-head CI and required reviews.

## Lean architecture gate

| Requirement | Current evidence and remaining work |
|---|---|
| One deterministic Rust authority; unmodified Hermes | Installed and exercised through planning, build, review and remediation. Hermes remains execution/projection, not the workflow authority. |
| Signed webhook intake plus bounded recovery polling | Live signed intake, deduplication, revocation and controller polling exercised. Keep these boundaries. |
| Dynamic bounded workflow; human-only merge | Live remediation is exercised. Alternate in-process direct execution and autonomous-merge coordinator/API were removed. Final readiness remains unproven live. |
| Policy-defined repositories, identities and exact models | Configured reviewer instances executed live without intentional substitution. Repository-scoped effect claiming is installed with cross-repository regression tests; multi-repository live operation is not proven. |
| Independent required and comparison reviews | Required and shadow results accepted independently; comparisons do not consume work-failure allowance. Required pending jobs have priority. An already-running comparison can still delay the serial worker. |
| Failure isolation and ordinary recovery | Completed-result collection precedes GitHub access. Confirmed non-start backoff and bounded audited retry exist. Per-case/capability isolation and ordinary pause/resume still need completion. |
| Immutable jobs across upgrades | Saved dispatch definitions are reused. Full preservation of actual execution settings and skill content across upgrades remains incomplete. Restrictive controls must still apply immediately. |
| Compact evidence with durable provenance | Installed exports deduplicate accepted event/run payloads and provide role-specific indexes into retained artifacts. Full history remains duplicated in saved definitions; finish compact job inputs. |
| Safe workers and storage | Real service-identity builder execution and controlled publication work; managed storage and credential isolation are installed. Fresh re-review must prove recent command/scratch fixes. Cleanup must remain independent of unrelated work. |
| Small packaging and operating surface | Signed install/rollback, policy preservation and schema-8 ordered migrations are verified. Remove obsolete operational scaffolding; normal operation must not require case-specific shell scripts. |
| Retire legacy Pip Python | Pending complete live cutover proof. Keep upstream Hermes and useful small probes/fixtures; remove the obsolete Pip runtime, packaging and CI rather than maintaining two implementations. |
| Final handoff | Pending exact-head readiness verification, documentation reconciliation and removal of temporary operator elevation. Never merge automatically. |

## Evidence boundaries

Pip release CI `34103318222` and signed build `34103318326` succeeded, including
Linux service lifecycle tests. Installing that cohort preserved the stopped
ledger and paused policy byte-for-byte. These checks prove this deployment, not
the entire workflow or all architecture rows above.

A subsequent local review-publication change lets either accepted lane publish
when the other App is unavailable, without accepting partial ledger evidence.
Its regression test, full Rust suite and Clippy passed; source `240ff50` is
pushed, but that change is not installed in this snapshot.

Historical installation, rollback, ingress and sandbox observations remain in
[`docs/evidence/`](evidence/). Re-read current code, ledger, service state, exact
PR head and GitHub checks before changing state or marking any gate complete.
Neither this checklist nor a green local test substitutes for missing live proof.
