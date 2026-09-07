# Pip implementation status

Updated 2026-09-07. **Not complete.** The target is
[pip-architecture-plan.md](pip-architecture-plan.md); the requirement-by-requirement
[completion audit](completion-audit.md) defines the remaining scope. This is a
current checkpoint, not a development log. Earlier investigations and failed
experiments remain in Git history and [evidence/](evidence/).

## Live checkpoint

Pirate was rechecked and runs `8d3dde498fe60da1b4be9b5f2728610e6cdedcdb`, ledger
schema 8. CI `34103318222` and signed deployment `34103318326` passed; installation
preserved the stopped ledger and paused policy byte-for-byte. The dedicated
controller, direct-worker and webhook-consumer timers are active. No `pip-worker`
process was running at inspection. Refresh this dated evidence before mutation.
Conversational Hermes is separate and untouched; Hermes remains upstream.

MDK #993 produced draft [PR #1726](https://github.com/marmot-protocol/mdk/pull/1726),
currently at `625bb4299187139461a64fd6eb5337ea35804261`.

- Planning, initial build, independent reviews, remediation and re-review have
  executed. Last ledger inspection: `FINAL_REVIEW`, revision 26, plan 1, policy 7,
  remediation round 1. Builder attempt 14 and its reported limitations are retained.
- Current-head required CI `34104104224` passes. App reviews `5130401272` and
  `5130401511` both approve this exact head; old-head reviews remain historical.
- Native general task `t_d2cdf42a` used `openai-codex/gpt-6-astra`; its retained
  logs showed 611 library and two sanitization tests passing. Required direct
  attempt 15 used `cursor/kimi-k3-max`, reporting successful tests and hostile-input
  probes. Its long-TMPDIR failure and short-path rerun remain disclosed. Shadow
  Opus attempt 16 approved but is advisory, not an additional required vote.
- Final holistic review has **not** started. GitHub reports `BLOCKED`: Safe Master
  requires signatures and both PR commits are unsigned. Commit email also maps
  to `pip`, not the configured `agent-p1p`. The PR remains open and draft.
  No merge occurred. Reinspect other merge blockers after signing.
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

## Source implemented, not deployed

All rows below are descendants of the installed release. Source tests do not
prove installation, Pirate migration or a live provider run.

| Change | Source | Remaining live proof or scope |
|---|---|---|
| Independent controller phase reporting | `07a0cd9` | Live outage drill; extended by the case-scoped source changes below |
| Attempt both accepted review-lane publications independently | `240ff50` | App-outage replay; acceptance still requires both required lanes |
| One installer unit list instead of repeated operations | `5ca4ae3` | Installation of the newer cohort |
| Case-scoped finding IDs, immutable schema-9 migration | `165af50` | Pirate migration with history preserved |
| One evidence bundle per frozen batch, exact job references, schema 10 | `8345aad` | Live dispatch; historical rows stay unchanged and handoff files still carry bounded inputs |
| Short private per-execution TMPDIR and cleanup | `a8bdb4c` | Live Cursor/provider execution |
| One authorization snapshot for decision and evidence; unrelated revocations survive an outage | `6917c81` | Extended by case-scoped advancement below; installation remains pending |
| Human-readable plans, reviews and PR descriptions | `1676a39` | Deployment; existing GitHub text has not been rewritten |
| Controller signing, retained source commits and exact source/published-head binding | `b326a0a` | Key registration, deployment, signed publication and fresh CI/reviews |
| Audited publication-only recovery of an accepted unsigned build | `36e3a66` | Deployment and live recovery |

The next source checkpoint replaces the repository-wide advancement veto with
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
malformed evidence. It is not deployed; global storage/queue capability failures,
startup credential dependencies and already-running comparisons remain separate
limitations, not evidence of complete failure isolation.

Signing integration `b326a0aff27c0ae1d46b94b95bb6d7aa6e2e3446` passed CI
`34123194095` and deployment build `34123194049`, including
`CONTROLLER_SIGNING_CREDENTIAL_SANDBOX_OK` under the real Linux service sandbox.
Recovery `36e3a66e67fb53676e957690408967f246862ba1` passed local publication,
builder/review recovery, CLI, signed final-preflight, state-machine and all-target
Clippy checks. CI `34125632649` and signed deployment `34125632730` both passed,
including the real root/queue/service-state checks, controller signing sandbox,
two-identity workspace handoff and install/reinstall/upgrade/rollback/restart gates.
The cohort is staged on Pirate at
`/home/jeff/.cache/pip-deployments/36e3a66e67fb53676e957690408967f246862ba1/run-34125632730/pip-release`.
Both local and already-installed Pirate verifiers accepted all 26 artifacts with
the permanent trust anchor: manifest
`16dce33b09a5b6b8fdbd2d27953c9052e872c1b6a29fad8b7c1caeb24297b652`, binary
`8a77497903d1e76e9fd2ad4663076c91e9ab6e856a74e1210244a88c4fb16327`.
This is verified staging, **not installation**; no service, policy or canary head
was changed by staging.

Human-facing GitHub text now shows verdict, reviewer/model, findings, suggestions,
reported checks and limitations. Structured results stay in the ledger, not JSON
comment blobs. No attachment service is needed for normal operation. Inspect
partially published content-bound effects before a formatting upgrade; never
bypass ownership/idempotency checks to accept a changed body.

## Signing prerequisite and recovery

Jeff approved a dedicated signing-only key, staged on Pirate, not in this repo:

- Private key: `/etc/pip/commit-signing/key`, root:root 0600 in a 0700 directory.
- Public fingerprint: `SHA256:uoZahdy4QImrKeSOfbsfX6FeweFeHC+eEJ2OXVlO7wg`.
- Identity: `/etc/pip/commit-signing/identity.json`, root:root 0600; actor
  `292420120`, name `Pip`, email `292420120+agent-p1p@users.noreply.github.com`.
- Identifier-only aliases: `/etc/credstore/pip-commit-signing` and
  `/etc/credstore/pip-commit-signing-identity`. Transient controller probes loaded
  both correctly; worker reads of both protected source files are denied.

The key is **not yet listed** on `agent-p1p`'s GitHub signing keys. The existing
repository token returned HTTP 403 for account-key registration. Do not broaden
that token, reuse Jeff's key, or register this as an authentication key. Jeff must
register the public half as a signing key; the older dual-purpose key is untouched.
Credential delivery is proven, not live GitHub commit verification.

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

1. Register the approved signing key; reverify the staged cohort before installation.
2. Install while paused with a fresh stopped-state backup. Verify schema 8 to 10
   and semantic history preservation; inspect partially published effects.
3. Authorize signed republication of the accepted canary, then exercise fresh CI,
   required reviews, final holistic review and human readiness. Reinspect actual
   mergeability; do not assume signatures were the only blocker or relax
   `blocked` blindly. Never merge automatically.
4. Finish the [lean architecture gates](completion-audit.md): case/capability
   isolation, immutable actual settings/skills, nonblocking comparisons, ordinary
   pause/resume, compact live dispatch and obsolete operational-path removal.
5. After live cutover proof, remove legacy Pip Python/runtime packaging/CI, keeping
   useful fixtures and upstream Hermes. Reconcile docs and revoke temporary sudo
   when no longer needed. Keep the full goal active until both the live PR and
   architecture requirements are verified.
