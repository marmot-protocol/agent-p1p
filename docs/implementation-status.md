# Pip implementation status

Updated 2026-09-07. This inventory distinguishes implementation, deployment and
live proof. The target is [the lean architecture](pip-architecture-plan.md).

## Latest verified live state

Pirate runs signed `8d3dde498fe60da1b4be9b5f2728610e6cdedcdb`, verified by
CI `34103318222` and deployment `34103318326`, including Linux lifecycle tests.
Installation preserved the stopped ledger and paused policy byte-for-byte.

- #993 is the sole labeled canary, with accepted plan 1.
  Draft [MDK #1726](https://github.com/marmot-protocol/mdk/pull/1726) now has
  remediation head `625bb4299187139461a64fd6eb5337ea35804261`; fresh GitHub CI
  `34104104224` passed. Earlier reviews remain bound only to head `05070de3`.
- GitHub reviews `5129269820` (general, changes requested) and `5129270044`
  (security/performance, approved) are published on the old `05070de3` head under the
  separate App identities. Both include reported verification and limitations.
  Required Kimi attempt 11 and shadow Opus attempt 12 disclosed denied commands;
  those old reports do not provide successful command-execution proof.
- The installed binding/retry fixes preserved six case-local failures and the
  completed remediation. Builder attempt 14 reverified and reused clean commit
  `625bb4299187139461a64fd6eb5337ea35804261`. Its result is accepted, with the
  case now `FINAL_REVIEW` at revision 26, plan 1 and remediation round 1. The worker
  reported passing CLI checks and `just fast-ci`, plus a Marmot parallel-test
  timeout whose isolated rerun passed. That limitation remains in its result.
- Publication succeeded: the old dispatcher used an ambiguous builder round
  counter while publication expected a different number. A regression-tested
  fix selects the exact run attached to the accepted `BUILD_RECORDED` event;
  new jobs also get an explicit one-based `build_round`. The accepted result
  and history were not rewritten, and no new model attempt was required. The
  first publication cycle hit a generic identity/head guard; its normal retry
  succeeded. GitHub and the ledger agree on the new head, with the builder's
  reported checks and limitations preserved in the PR body.
- Fresh re-review passed: native task `t_d2cdf42a` used configured
  `openai-codex/gpt-6-astra`; direct attempt 15 used required
  `cursor/kimi-k3-max`. Both approved head `625bb429`. App reviews `5130401272`
  and `5130401511` are published on that exact head. Native retained logs and
  their reported hashes were independently checked: 611 library tests and two
  sanitization integration tests passed. Kimi reported successful tests and
  hostile-input probes; its first long-TMPDIR test failure and short-path rerun
  remain disclosed. Shadow comparison attempt 16 subsequently completed with
  `APPROVE`; it is advisory evidence, not an additional required approval.
- Final preflight is pending, not accepted; the holistic final reviewer has not
  started. GitHub reports conflict-free `mergeable=true` but merge state
  `blocked`. Safe Master requires signatures and both PR commits are unsigned.
  Commit email also maps to `pip`, not the configured `agent-p1p` account.
  Signing/publication identity must be corrected without giving workers GitHub
  credentials or weakening branch rules. Any replacement head requires fresh
  CI and reviews. The registered Pip signing public key was located; the private
  key was not found in the scoped standard Pirate locations. Its location was
  requested from Jeff; no private keys were read, created or registered.
  GitHub's documented [`createCommitOnBranch` signing API](https://docs.github.com/en/graphql/reference/commits#createcommitonbranch)
  may avoid a separate signing key. This is an investigation, not live proof:
  file-mode support, exact tree/parent/actor verification, recovery and the
  source-build-to-published-head binding must be established before adoption.
  A read-only GraphQL probe using Pirate's existing credential confirmed
  `agent-p1p` (numeric actor 292420120) and WRITE access to MDK. Inspection of
  the accepted canary range found only ordinary file additions/modifications
  (mode 100644), with original parent
  `897111a9d9a9772b824cb4ed0ff60b9cb1242f5f`. No signing mutation or PR rewrite
  has occurred. This capability check does not prove server-side signing or
  authorize treating a different signed commit as the accepted builder SHA.
- #891, #1228 and #1639 are abandoned with history retained.
- Hermes remains the upstream installation; Pip has not introduced a fork.
  Conversational Hermes was untouched. Pip execution timers and its dedicated
  gateway are active under approved policy 7; ingress remains active.

These are dated observations, not a promise that a process is still running.
Inspect the current ledger, services and GitHub evidence before acting.

## Implemented and deployed

- One Rust workflow authority, unmodified Hermes, webhook intake plus polling,
  explicit models and human-only merge.
- Removed the alternate in-process direct executor and autonomous-merge
  coordinator/API; moved useful tests onto the production queue boundary.
- Detached review failures do not consume the case work budget. Cleanup errors
  do not block otherwise healthy storage. Reviewer credentials are acquired
  only when publication needs them.
- Confirmed runtime non-starts have durable exponential backoff, separate from
  work failures. Uncertain handoffs remain fenced against duplicate execution.
- Dispatch retries reuse saved task definitions rather than rebuilding them
  from current release defaults. Full saved-job compatibility remains incomplete.
- Hermes settings are verified through structured configuration output.
- Worker sandboxes support Node JIT while preserving controller/ingress
  restrictions. Native Hermes sees only its required runtime paths and cannot
  reach the controller ledger through same-user process aliases.
- Shared Git indexes survive controller inspection under a restrictive umask.
  Supported builder recovery preserves descendant commits and unfinished edits,
  with no source reset or failure-history rewrite.
- Normal upgrades preserve operator policy; only first installation seeds an
  inert policy. Release permissions are independent of the caller's umask.
- Cursor receives its retained prompt through file-backed stdin. One strict,
  bound result may follow progress commentary; ambiguous contracts fail closed.
- Precise worker field types and a credential-free `validate-worker-result`
  command catch schema errors before submission. Direct role instructions no
  longer ask for unavailable Hermes completion tools.
- Read-only `status --case` and `status --attempt`, plus bounded audited
  `authorize-builder-retry`, replace handwritten inspection/reset operations.

These changes have Rust regression coverage and signed-release Linux lifecycle
coverage. They do not by themselves establish end-to-end success.

## Result compatibility and remaining work

Both adapters separate result retention from workflow advancement. Paused
collection uses accepted policy, retains late results without live effect leases,
and does not publish or dispatch. Live native and direct retention and direct
resume are proven above. One shared generation check accepts peer-only review
progress while fencing new CI, retries and terminal dispositions. Shadow Opus
was recorded as a comparison after the required lane advanced. Active-cycle
failure isolation and full saved-policy execution compatibility remain incomplete.
The installed runtime moves that credential-free collection phase ahead of GitHub
dependencies even during active operation. A real CLI regression with missing
GitHub credentials proves completed direct work is retained without advancing
the case; a real live-outage drill remains unproven.

Cursor now approves verification commands noninteractively, with an unchanged
checkout postcondition and the credential-isolating service sandbox. This is
not an OS-enforced read-only source mount. Fresh Kimi review reports successful
command execution; old reports are not retroactively upgraded. Published
reviews now expose suggestions and reviewer-reported evidence and limitations.

Review retry shares the bounded root/offline recovery path and preserves the
accepted PR/head/build. Queue backpressure admits one serial handoff at a time;
tests cover restart, expired handoffs and recovery without duplicate execution.
Both are installed. Required jobs now take priority over pending comparisons
regardless of effect ID, with queue regression coverage. An already-running
comparison can still occupy the serial worker; that isolation remains unfinished.
Ordinary pause/resume must stop requiring policy-copy commands.

New native scratch schema 3 reserves space for MDK's private Unix-socket staging
path; schemas 1 and 2 remain readable without rewriting existing task storage.
Local socket/retirement tests, a real MDK socket probe and signed Linux gates
pass. The fresh native review above also proved the layout in its actual sandbox.

Jobs reference a retained SHA-256-bound evidence file; new Cursor files support
bounded line reads. Existing jobs keep their artifact bytes and references.
Transport tests cover large histories and drift. The installed format-2 export removes identical
accepted-result copies from events using exact run/digest references; stored
ledger records and existing format-1 jobs remain unchanged. This is a bounded
deduplication improvement. A subsequent locally tested role-specific index
points into the same artifact without copying payloads. It selects the current
plan, exact-head build/CI, applicable findings and final-review evidence;
independent reviewer indexes omit peer verdicts. Full history stays readable.
This index is installed. A subsequent local storage change keeps each distinct
evidence bundle once per frozen dispatch batch. Persisted native projections
and direct outbox messages reference that batch's exact job definition instead
of copying it. Store reads verify and resolve those references for the existing
adapters, so the worker contract and execution inputs do not change. Older
inline batches, projections and messages remain readable and replay without
rewriting their bytes. Schema 10 fences older binaries from interpreting the
new representation; it adds no tables and does not rewrite historical rows.
Tests cover exact replay/restart, old schema-9 records, evidence/hash corruption,
foreign cases, wrong transports, and rejection before committing a lease.
The three-reviewer fixture retains less than one quarter of its previous
definition/output JSON size. Full evidence remains available, and controller-to-
worker handoff files still carry their bounded inputs; this is not removal of
the retained history or a claim that existing deployed records were compacted.
The full Rust suite, Clippy, formatting and shell syntax checks pass for this
storage change, including native/direct dispatch and result-collection tests.
New Linux release validation and installation remain separate gates. Outbox
payload digests are checked before a claim commits, so corrupted references
cannot strand a committed work lease. This change is not installed.
The product-naming test has moved
from Python into Rust in preparation for legacy runtime removal after cutover.

## Remaining completion gates

Further local simplification keeps schema 8 unchanged while replacing eight
repeated migration blocks with one ordered, individually transactional loop.
Repository-scoped effect selection now precedes leasing in every policy-owned
consumer. A regression reproduced foreign-repository dispatch before the fix;
dispatch, restart/lease isolation, the full Rust suite and Clippy now pass.
These changes are now installed. They do not yet isolate every
failed case within one repository: authorization and phase errors can still
stop an otherwise unrelated active cycle.

A subsequent local controller change reports capability failures individually
and continues unrelated phases. Authorization, takeover and operational-bound
errors still block advancement; storage readiness still gates new work. The
real controller regression covers healthy idle operation plus intake, storage,
queue, publication and authorization failures without external writes or case
history changes. Degraded cycles emit their complete JSON report before exiting
nonzero, including when paused collection fails. This is phase isolation, not
yet per-case authorization isolation. The full Rust suite, Clippy and formatting
checks pass; the change is not installed in this snapshot.
The service unit's absolute `LoadCredential` paths also remain startup
dependencies; missing App files can still stop the process before collection.

Installer simplification now uses one ten-unit list for reads, comparisons,
snapshots and writes instead of repeating those operations per file. Expanded
lifecycle tests change every fixture unit across upgrades and verify all bytes
and modes, including rollback at every existing injected-failure point. The
focused lifecycle suite passes before and after the behavior-preserving change;
the full Rust suite, Clippy and formatting checks also pass. Signed deployment
remains a separate gate; the live re-review was not interrupted for this change.

Source `5ca4ae3` passed CI `34106970663` and signed deployment build
`34106970547`; Pirate remains on `8d3dde4`. A subsequent local final-gate change
distinguishes merge conflicts, unknown mergeability and GitHub's observed merge
state instead of one generic blocker. It still requires both conflict-free and
`clean`; it does not infer a specific failed branch rule from `blocked` alone.

Schema 9 narrows the finding primary key from globally unique `finding_id` to
`(case_key, finding_id)`. A regression reproduced unrelated issues failing on
the same reviewer-chosen ID. The migration copies existing payloads, digests,
origins and heads unchanged, restores immutability triggers, and leaves
same-case duplicates as transaction failures. Tests cover separate issues and
repositories, replay, restart, schema-8 migration and schema-1 forward upgrade.
The full Rust suite, Clippy and formatting checks pass, including all 13 local
installer lifecycle tests. Signed Linux lifecycle verification and installation
remain separate gates; the Pirate ledger remains schema 8.
The first schema-9 CI/deployment runs (`34108834809`, `34108834874`) failed at
an old schema-8 assertion in `tests/lifecycle/run-in-container.sh`. That separate
assertion was corrected in `165af50`; no failed cohort was deployed.
CI `34109634658` and signed deployment build `34109634710` for that correction
subsequently passed, including the Linux lifecycle checks.

A subsequent local TDD change attempts both accepted review-lane publications
even if one App is unavailable. The ledger advances only after both succeed;
partial external success retains the stable markers for retry. The regression
covers either or both lanes failing, lease release and no partial acceptance.
The full Rust suite and Clippy pass; this change is not yet deployed.

### Direct worker temporary storage (local, 2026-09-07)

Cursor executions now allocate a fresh short mode-0700 directory under the
service's existing private `/tmp`, setting `TMPDIR`, `TMP` and `TEMP` explicitly.
The prompt and invocation record identify the allocation. It is disposable;
build caches remain in the managed workspace and accepted artifacts remain in
the run artifact directory. Ordinary return paths remove the allocation,
including provider launch failure, timeout and malformed output. Abrupt service
termination still relies on the service-private temporary namespace lifecycle.
Allocation errors are runtime unavailability, not a failed attempt to solve an
issue. No unit sandbox protection is weakened.

The regression first reproduced the inherited long-path failure, then caught
default temporary-directory permissions being broader than 0700. The corrected
implementation requests private permissions at creation. Tests bind an actual
Unix socket in nested test storage, verify per-execution isolation, and verify
cleanup after success and failures. Executor and production adapter tests pass.
The full workspace Rust suite, Clippy with warnings denied, formatting and diff
checks also pass (`/tmp/pip-direct-temp-validation-20260907.log`).
This change is not installed and has not been exercised by a live provider.

1. Finish one real issue through builder, exact-head CI, all required independent
   reviews, remediation where needed, and final human-ready disposition.
2. Finish capability/case failure isolation and safe result collection during
   pause; extend confirmed-non-start classification where still missing.
   Extend live peer-review ordering proof to the reverse completion order.
3. Preserve saved job settings/skills through execution and acceptance across
   upgrades. Apply pause/revocation immediately without rewriting old jobs.
4. Replace growing full-history prompts with compact role-specific inputs and
   retained immutable evidence.
5. Keep storage and ordinary recovery small and supported; remove obsolete paths
   and operational scaffolding.
6. After live cutover proof, delete the legacy Python runtime and obsolete
   CI/docs, retaining useful parity fixtures and history in Git.
7. Verify the exact PR head, CI and required reviews; reconcile documentation;
   revoke temporary operator elevation when no longer needed. Do not merge.

Keep the complete goal active until both the slimmed architecture and live PR
are verified. See `docs/evidence/` for historical observations, not current
deployment authority or reusable case-specific commands.
