# Pip implementation status

Updated 2026-09-07. **Not complete.** The target is
[pip-architecture-plan.md](pip-architecture-plan.md); the requirement-by-requirement
[completion audit](completion-audit.md) defines the remaining scope. This is a
current checkpoint, not a development log. Earlier investigations and failed
experiments remain in Git history and [evidence/](evidence/).

## Live checkpoint

Pirate runs `5d4fdb2c3a9d14e84d3c5444c31bdfaaa5750397`, ledger schema 10.
CI `34133415495` and signed deployment `34133415508` passed. The latest install
preserved the complete logical ledger dump and active policy bytes. The dedicated
controller, direct-worker and webhook-consumer timers are resumed, with repeated
firings and finite next runs verified. See the
[signed recovery evidence](evidence/2026-09-07-signed-canary-recovery.md).
Conversational Hermes is separate and untouched; Hermes remains upstream.

MDK #993 produced draft [PR #1726](https://github.com/marmot-protocol/mdk/pull/1726),
currently at signed head `53ac3d8f8143ea9f186bf677f59c3a16f2632fca`.

- Planning, initial build, independent reviews, remediation and re-review have
  executed on the accepted unsigned build. Publication-only recovery retained
  that exact tree and produced a GitHub-verified signed replacement. Last ledger
  inspection: `WAITING_CI`, revision 28, plan 1, policy 7, remediation round 1.
  Builder attempt 14 and its reported limitations are retained; no builder rerun.
- Fresh CI `34134517302` is running. Earlier CI `34104104224` and App approvals
  `5130401272` / `5130401511` bind the old unsigned head, not the new head.
  Required reviews must run again after CI passes.
- Native general task `t_d2cdf42a` used `openai-codex/gpt-6-astra`; its retained
  logs showed 611 library and two sanitization tests passing. Required direct
  attempt 15 used `cursor/kimi-k3-max`, reporting successful tests and hostile-input
  probes. Its long-TMPDIR failure and short-path rerun remain disclosed. Shadow
  Opus attempt 16 approved but is advisory, not an additional required vote.
- Final holistic review has **not** started. The signed commit is verified for
  `agent-p1p`; signatures no longer need operator setup. The PR remains open and
  draft with fresh checks/reviews pending. No merge occurred. Reinspect actual
  mergeability after the new head's gates complete.
- #891, #1228 and #1639 are abandoned with history retained.

Models come from validated policy: planner/general/final GPT-6 Astra, builder
Grok 4.6 high/fast, required security/performance Kimi K3 Max, and shadow Opus 5
thinking/high. Use exact policy/provider identifiers, never substitute models.

## Implemented and deployed

- One deterministic Rust authority, unmodified Hermes execution/projection,
  signed webhook intake plus bounded polling, policy identities and human merge.
  The alternate direct executor and autonomous-merge coordinator/API were removed.
- Result retention is separate from advancement. Paused collection retains work
  without dispatch/publication. Confirmed non-starts have backoff distinct from
  work failures; uncertain handoffs remain fenced.
- Saved dispatch definitions survive retry. Role-specific indexes and format-2
  exports avoid repeated event/run payloads; independent reviewer indexes omit
  peer verdicts. Full immutable evidence remains available.
- Repository-scoped effects are selected before leasing. Required pending work
  precedes pending comparisons; comparison failures do not consume work budget.
  Already-running comparisons can still delay required work.
- Managed scratch/storage, two-identity Git handoff, Node JIT-compatible sandboxes,
  native scratch schema 3 and real reviewer commands are exercised. Unchanged
  checkout checks are not an OS-enforced read-only source mount.
- Policy-preserving install/rollback, umask-safe release permissions, one ordered
  migration loop, strict results, read-only case/attempt status and audited
  builder/review retries replace earlier ad hoc paths.

These are partial gates, not end-to-end or complete lean-architecture acceptance.
JG's latest direction is to finish the working canary before further cleanup;
the remaining architectural work is deferred, not claimed complete.

## Recent deployed changes and remaining live proof

The changes below are included in the installed release. Installation and healthy
idle reconciliation do not prove every outage/replay path or a live provider run.

| Change | Source | Remaining live proof or scope |
|---|---|---|
| Independent controller phase reporting | `07a0cd9` | Live outage drill; extended by the case-scoped source changes below |
| Attempt both accepted review-lane publications independently | `240ff50` | App-outage replay; acceptance still requires both required lanes |
| One installer unit list instead of repeated operations | `5ca4ae3` | Installed; current services resumed successfully |
| Case-scoped finding IDs, immutable schema-9 migration | `165af50` | Pirate migration verified with all existing table data preserved |
| One evidence bundle per frozen batch, exact job references, schema 10 | `8345aad` | Schema installed; new live dispatch remains unproven; historical rows stay unchanged |
| Short private per-execution TMPDIR and cleanup | `a8bdb4c` | Live Cursor/provider execution |
| One authorization snapshot for decision and evidence; unrelated revocations survive an outage | `6917c81` | Installed with case-scoped advancement; live outage drill remains |
| Human-readable plans, reviews and PR descriptions | `1676a39` | Signed publication updated the PR description; historical review comments remain unchanged |
| Controller signing, retained source commits and exact source/published-head binding | `b326a0a` | Signed live publication verified on GitHub; fresh CI/reviews remain |
| Audited publication-only recovery of an accepted unsigned build | `36e3a66` | Exercised live without a new builder attempt; history retained |
| Case-scoped advancement, result selection and required-work scheduling | `8ca4e74` | Healthy format-2 cycles observed; live peer-outage drill remains |
| Optional controller credential startup | `5d4fdb2` | Linux absent/partial/full credential checks passed; installed controller published successfully; live App-outage test deferred |

The installed controller replaces the repository-wide advancement veto with
explicit case selection for authorization, bounds, takeover, result acceptance,
CI, publication and dispatch. SQL selects ownership before leasing or decoding
projection payloads; direct collection selects the durable attempt's case before
reading its result. Invalid results remain retained and reported for their owner.
Malformed jobs release their pre-execution lease without starting an attempt.
One scheduler considers required jobs across authorized cases before comparisons.
Active `controller-cycle` reports use format 2: per-case observations in `cases`
and shared queue selection in `direct_dispatch`; paused reports are unchanged.
This adds no schema or service. Local regression tests cover cross-case selection,
authorization outages, healthy CI advancement, required-work priority and retained
malformed evidence. Global storage/queue capability failures, startup credential
dependencies and already-running comparisons remain separate
limitations, not evidence of complete failure isolation.

The installed cohort passed Linux root/queue/service-state checks, controller
signing credentials/sandbox, two-identity workspace handoff and
install/reinstall/upgrade/rollback/restart gates. The full Rust suite passed on
the exact source in CI; final local direct-worker tests and all-target/all-feature
Clippy passed as well. Its 26 artifacts verified locally
and through Pirate's already-installed verifier/permanent trust anchor before
installation. Manifest: `00e3b092ccbef83d546c2f98c96a9c371cf96a4c40fe4eea90b963412913ade4`;
binary: `381a7dcdb4033de42ac0a1da1a4ed04f39701c5097f6cd583f59ccbfcade8e4b`.
These checks do not prove signed live publication or completion of the canary.

Human-facing GitHub text now shows verdict, reviewer/model, findings, suggestions,
reported checks and limitations. Structured results stay in the ledger, not JSON
comment blobs. No attachment service is needed for normal operation. Inspect
partially published content-bound effects before a formatting upgrade; never
bypass ownership/idempotency checks to accept a changed body.

## Signing prerequisite and recovery

Installed change: the controller unit uses namespaced credential-store
lookups for its GitHub token and review App metadata/keys, extending the existing
optional signing lookup. Missing credentials must not prevent service startup;
the consuming capability still fails closed. Pirate's five root-owned aliases
are prepared under `/etc/credstore`, with the existing 0600 source files unchanged
and worker reads denied. Installed `5d4fdb2` uses these aliases. A transient network-disabled Pirate
service successfully read all five aliases as `pip-control` without exposing
credential contents; this is lookup/delivery proof, not a deployed controller
outage drill. The disposable Linux lifecycle suite also passed startup with no
credentials, token only, and all five credentials. Its main service process
checked exact fake credential contents before executing the real paused
controller; worker reads of the source files were denied. Install, reinstall,
upgrade, rollback and restart recovery passed in the same run. Active review
publication during an outage remains a separate live gate. See the migration mappings in the
[deployment runbook](runbooks/deployment.md); startup tests do not prove a live
review publication.

Jeff approved a dedicated signing-only key, staged on Pirate, not in this repo:

- Private key: `/etc/pip/commit-signing/key`, root:root 0600 in a 0700 directory.
- Public fingerprint: `SHA256:uoZahdy4QImrKeSOfbsfX6FeweFeHC+eEJ2OXVlO7wg`.
- Identity: `/etc/pip/commit-signing/identity.json`, root:root 0600; actor
  `292420120`, name `Pip`, email `292420120+agent-p1p@users.noreply.github.com`.
- Identifier-only aliases: `/etc/credstore/pip-commit-signing` and
  `/etc/credstore/pip-commit-signing-identity`. Transient controller probes loaded
  both correctly; worker reads of both protected source files are denied.

JG registered the key as signing-key ID `1161893` on `agent-p1p`. Its API public
key exactly matches the approved public half on Pirate. Signed commit
`53ac3d8f8143ea9f186bf677f59c3a16f2632fca` is GitHub-verified for that account.
The older dual-purpose key is untouched; no token scopes were broadened.

Signing uses the exact accepted tree and validated parent, verifies the signature,
retains the source and records its published-SHA mapping. Workers receive no key.
Builder resolutions may join to their source build; CI, approvals and origin
confirmations still require the published head.

`authorize-publication-retry` shares root/inert-policy/stopped-runtime/drained-queue
checks with existing recovery. It appends one authorization, grants zero model
attempts and preserves accepted runs, plan, remediation round and deadline.
Unsigned-range recovery signs on the original planned base, not the old unsigned
PR head. Normal publication uses an exact-old-head lease and requires fresh CI
and reviews. Identical requests replay; stale/conflicting requests and unsigned
publisher results are rejected. See [the recovery runbook](runbooks/builder-recovery.md).

## Next gates

1. Follow fresh CI, required reviews, final holistic review and human readiness
   on the signed canary. Reinspect actual mergeability; do not assume signatures
   were the only blocker or relax `blocked` blindly. Never merge automatically.
2. After the working canary, revisit deferred [lean architecture gates](completion-audit.md): case/capability
   isolation, immutable actual settings/skills, nonblocking comparisons, ordinary
   pause/resume, compact live dispatch and obsolete operational-path removal.
3. After live cutover proof, remove legacy Pip Python/runtime packaging/CI, keeping
   useful fixtures and upstream Hermes. Reconcile docs and revoke temporary sudo
   when no longer needed. Keep the full goal active until both the live PR and
   architecture requirements are verified.
