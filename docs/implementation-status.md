# Pip implementation status

Updated 2026-09-07. This inventory distinguishes implementation, deployment and
live proof. The target is [the lean architecture](pip-architecture-plan.md).

## Latest verified live state

Pirate runs signed `e4106a988a70d8c76d31f6c3fedc86f5fc3048cf`, verified by
CI `34100251695` and deployment `34100251650`, including Linux lifecycle tests.
Installation preserved the stopped ledger and paused policy byte-for-byte.

- #993 is the sole labeled canary, with accepted plan 1 and builder attempt 9.
  Draft [MDK #1726](https://github.com/marmot-protocol/mdk/pull/1726) remains at
  `05070de3ef5e151bba702f85fd4c9e510f7e0df4`, with required GitHub CI green.
- The supported review retry preserved the build, five case-local failed
  attempts and prior escalation. Fresh same-head CI retained a separate
  observation and dispatched the review generation at revision 14.
- Paused collection retained completed general review `t_6b15c1cd` in the
  ledger without a case transition; repeat collection was an idempotent no-op.
  That old-generation result is evidence, not an accepted current review. It
  found unsanitized daemon error writers; its root-only recovery copy remains.
- General task `t_9bb53a26` was accepted as `REQUEST_CHANGES`. Required Kimi
  attempt 11 and shadow Opus attempt 12 completed; both disclosed denied
  verification commands. The new release retained both while paused without
  advancing the case. Resume accepted Kimi after the general review's revision.
- GitHub reviews `5129269820` (general, changes requested) and `5129270044`
  (security/performance, approved) are published on that exact head under the
  separate App identities. Both include reported verification and limitations.
  The case advanced to `REMEDIATING` revision 17. Builder attempt 13 created
  clean local commit `625bb4299187139461a64fd6eb5337ea35804261`, but result
  acceptance incorrectly compared its new commit with the incoming PR head
  (and required a PR number absent from the builder output contract). It failed
  and the case escalated at revision 18; all six case-local failures remain.
  The commit is retained, not published. A supported retry must reverify it.
  Regression tests now distinguish incoming planner/builder context from exact
  review/final output bindings. Full Rust tests, Clippy and signed Linux gates
  pass; this fix is installed but a real accepted remediation remains unproven.
  The same regression pass found the offline builder-retry guard only supported
  pre-PR failures. It now preserves an existing PR/head and remediation round,
  permits prior accepted builds, and still fences running or completed target
  attempts. The supported retry applied successfully at revision 19, preserving
  PR 1726, its old head, plan 1 and round 1. Normal execution has resumed.
- #891, #1228 and #1639 are abandoned with history retained.
- Hermes remains the upstream installation; Pip has not introduced a fork.
  Conversational Hermes was untouched. Pip execution timers, the dedicated
  gateway and ingress are active.

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
not an OS-enforced read-only source mount. The next review round must prove
real command execution; old reports are not retroactively upgraded. Published
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
pass. The next native review must prove the new layout in its actual sandbox.

Jobs reference a retained SHA-256-bound evidence file; new Cursor files support
bounded line reads. Existing jobs keep their artifact bytes and references.
Transport tests cover large histories and drift, but job definitions still
include full history. The installed format-2 export removes identical
accepted-result copies from events using exact run/digest references; stored
ledger records and existing format-1 jobs remain unchanged. This is a bounded
deduplication improvement. A subsequent locally tested role-specific index
points into the same artifact without copying payloads. It selects the current
plan, exact-head build/CI, applicable findings and final-review evidence;
independent reviewer indexes omit peer verdicts. Full history stays readable.
This index still requires deployment; eliminating full-history duplication in
saved job definitions remains unfinished. The product-naming test has moved
from Python into Rust in preparation for legacy runtime removal after cutover.

## Remaining completion gates

Further local simplification keeps schema 8 unchanged while replacing eight
repeated migration blocks with one ordered, individually transactional loop.
Repository-scoped effect selection now precedes leasing in every policy-owned
consumer. A regression reproduced foreign-repository dispatch before the fix;
dispatch, restart/lease isolation, the full Rust suite and Clippy now pass.
These changes still need signed deployment. They do not yet isolate every
failed case within one repository: authorization and phase errors can still
stop an otherwise unrelated active cycle.

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
