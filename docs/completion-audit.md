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

Signed Pip `7700a21c47368e270bb736f9efc7512e44f87fcd` is installed on Pirate;
CI `34138052044` and deployment `34138051885` passed. The latest installation
preserved the complete logical ledger dump and policy bytes. Schema remains 10.

The unsigned build passed CI `34104104224`, native general task `t_d2cdf42a`
and Kimi direct attempt 15. App approvals `5130401272` and `5130401511` bind
that historical head. JG has now registered the dedicated signing key, and
publication-only recovery produced signed head
`53ac3d8f8143ea9f186bf677f59c3a16f2632fca` on the original planned base.
GitHub verifies its signature for `agent-p1p`; its tree exactly matches the
accepted unsigned build. No new builder attempt was granted or run.

The case is `SHADOW_READY`, revision 37. Signed-head CI `34134517302` passed,
fresh required App approvals `5133675338` / `5133675731` were published, and
final task `t_71f141df` returned `READY` after live preflight passed. Pip published
its human-held readiness notification. The PR remains draft and no merge occurred. See the
[signed recovery evidence](evidence/2026-09-07-signed-canary-recovery.md).

Live acceptance gates (satisfied for this signed head):

1. Required CI must be green on the signed current head; any further head change must repeat it.
2. All required independent re-reviews on that same head, including origin
   confirmation of the blocking finding's resolution. Actual Cursor verification
   commands and the native review scratch layout must work in their sandboxes.
3. Current authorization, ownership, clean mergeability, and no unresolved
   blocking reviews or threads in the final preflight.
4. A fresh holistic final review and a published human-held readiness result.
5. Any further remediation must repeat exact-head CI and required reviews.

## Lean architecture gate

JG's latest direction is to finish the existing canary before further cleanup.
The remaining cleanup below is deferred, not reported as completed.

| Requirement | Current evidence and remaining work |
|---|---|
| One deterministic Rust authority; unmodified Hermes | Installed and exercised through planning, build, review and remediation. Hermes remains execution/projection, not the workflow authority. |
| Signed webhook intake plus bounded recovery polling | Live signed intake, deduplication, revocation and controller polling exercised. Keep these boundaries. |
| Dynamic bounded workflow; human-only merge | Live remediation and final human-held readiness are proven. Alternate in-process direct execution and autonomous-merge coordinator/API were removed. |
| Policy-defined repositories, identities and exact models | Configured reviewer instances executed live without intentional substitution. Repository-scoped effect claiming is installed with cross-repository regression tests; multi-repository live operation is not proven. |
| Independent required and comparison reviews | Required and shadow results accepted independently; comparisons do not consume work-failure allowance. Required pending jobs have priority. An already-running comparison can still delay the serial worker. |
| Failure isolation and ordinary recovery | Completed-result collection precedes GitHub access. Confirmed non-start backoff and bounded audited retry exist. Installed code scopes checks, acceptance and effects by case before selection; regression tests cover unavailable peers, malformed retained results and required-work priority across cases. Healthy live cycles pass; live fault-injection proof, shared capability/startup isolation and ordinary pause/resume remain incomplete. |
| Immutable jobs across upgrades | Saved dispatch definitions are reused. Full preservation of actual execution settings and skill content across upgrades remains incomplete. Restrictive controls must still apply immediately. |
| Compact evidence with durable provenance | Installed exports deduplicate accepted event/run payloads and provide role-specific indexes. Schema-10 storage retains each distinct input bundle once per frozen batch and references its exact jobs from persisted projections/outbox messages. The live upgrade preserved legacy records. Rust tests, adapter compatibility, corruption/replay tests, Clippy and Linux release validation pass; fresh required, comparison and final jobs completed live. |
| Safe workers and storage | Real service-identity builder execution and controlled publication work; managed storage and credential isolation are installed. Fresh native re-review and final review ran tests successfully in their scratch layouts. The installed direct adapter's private short temporary storage was exercised by successful required and comparison provider runs. Cleanup must remain independent of unrelated work. |
| Small packaging and operating surface | Signed install/rollback, active-policy preservation and ordered migrations through schema 10 are verified. Remove obsolete operational scaffolding; normal operation must not require case-specific shell scripts. |
| Retire legacy Pip Python | Pending complete live cutover proof. Keep upstream Hermes and useful small probes/fixtures; remove the obsolete Pip runtime, packaging and CI rather than maintaining two implementations. |
| Final handoff | Exact-head human-held readiness verified and documentation reconciled. No automatic merge. Temporary operator elevation is removed at handoff. |

## Evidence boundaries

Pip release CI `34103318222` and signed build `34103318326` succeeded, including
Linux service lifecycle tests. Installing that cohort preserved the stopped
ledger and paused policy byte-for-byte. These checks prove this deployment, not
the entire workflow or all architecture rows above.

A subsequent installed review-publication change lets either accepted lane publish
when the other App is unavailable, without accepting partial ledger evidence.
Its regression test, full Rust suite and Clippy passed; source `240ff50` is
pushed and included in `8ca4e74`; a live App-outage replay remains unproven.

Historical installation, rollback, ingress and sandbox observations remain in
[`docs/evidence/`](evidence/). Re-read current code, ledger, service state, exact
PR head and GitHub checks before changing state or marking any gate complete.
Neither this checklist nor a green local test substitutes for missing live proof.
