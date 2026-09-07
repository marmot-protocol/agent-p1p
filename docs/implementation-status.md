# Pip implementation status

Updated 2026-09-07. This inventory distinguishes implementation, deployment and
live proof. The target is [the lean architecture](pip-architecture-plan.md).

## Latest verified live state

Pirate runs signed `2efc26539f0662dbbd0c79c4a64f068b66fd5004`, verified by
CI `34091529650` and deployment `34091529622`, including Linux lifecycle tests.
Installation preserved the stopped ledger and paused policy byte-for-byte.

- #993 is the sole labeled canary, with accepted plan 1 and builder attempt 9.
  Draft [MDK #1726](https://github.com/marmot-protocol/mdk/pull/1726) remains at
  `05070de3ef5e151bba702f85fd4c9e510f7e0df4`, with required GitHub CI green.
- The supported review retry preserved the build, five case-local failed
  attempts and prior escalation. Fresh same-head CI now retains a separate
  observation and advanced the case to `REVIEWING` revision 14.
- Paused collection retained completed general review `t_6b15c1cd` in the
  ledger without a case transition; repeat collection was an idempotent no-op.
  That old-generation result is evidence, not an accepted current review. It
  found unsanitized daemon error writers; its root-only recovery copy remains.
- After restoring active policy 7, general task `t_9bb53a26` started in upstream
  Hermes with the short scratch layout. Cursor is executing required Kimi
  attempt 11 with `--trust`; shadow attempt 12 is queued behind it. Ledger
  `RUNNING` means leased, not necessarily an executing provider process.
  Required review completion, remediation and final readiness remain unproven.
- #891, #1228 and #1639 are abandoned with history retained.
- Hermes remains the upstream installation; Pip has not introduced a fork.
  Conversational Hermes was untouched. Pip execution timers and ingress are active.

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

Direct result collection is deployed separately from workflow advancement.
The paused controller can retain an exactly bound completed result without
credentials, publication, a new task, or a case transition. Resume accepts the
saved result without rerunning the provider. Tests cover restart, expired
dispatch lease after retention, malformed bindings, failures and queue traversal.
Native Hermes collection now has local coverage for the same pause/restart
behavior using the existing immutable evidence table, not another workflow
database. A broken direct queue does not prevent native collection while paused.
Historical-policy collection is deployed and proven on the retained live review.
Active-cycle/cross-case failure isolation and saved-policy execution compatibility
still need work; not all pause semantics are done.
Collection also retains late completions from held/superseded work as evidence
without advancing those cases. The live general reviewer returned an accepted
`REQUEST_CHANGES` on PR #1726 head `05070de3`. Both Cursor executions completed,
but their reports disclosed denied verification commands. Their queue results
remain preserved on disk; installed collection rejected the required result
after the general reviewer advanced the case revision. The runtime is paused.

Follow-up regression coverage separates direct-result retention from live
effect leases and shares the native/direct generation check: peer-only reviews
may advance, but new CI, retries and terminal dispositions fence old results.
Retention uses accepted policy rather than today's changed model configuration.
Cursor review commands now receive noninteractive approval, with an unchanged
checkout postcondition and the existing credential-isolating service sandbox;
this is not an OS-enforced read-only source mount. Evidence files support
bounded line reads. Published reviews retain each reviewer's suggestions and
reported verification/limitations instead of only the verdict. These fixes
await signed deployment and live proof.

Review-start recovery now shares the existing bounded root/offline recovery
path: `authorize-review-retry` preserves the accepted PR/head/build and creates
fresh CI observation, not a builder rerun or approval. Short scratch schema-2
paths now pass a real Unix-socket allocation test while old schema-1 paths stay
unchanged. These changes have local Rust/Clippy and signed Linux lifecycle
coverage and are installed. The recovery and fresh same-head CI succeeded on #993.
Native result ingestion now also accepts a reviewer after peer-only
`REVIEW_RECORDED` revisions. A regression joins both real-shaped review
contracts through final-review state; intervening CI/retry generations still
fence older work. This ordering fix is installed but still awaits live proof.

Local queue backpressure now keeps only one outstanding handoff for the serial
Cursor service, rather than starting leases for jobs waiting behind it. Existing
multi-job queues still drain; an expired uncertain handoff cannot be replaced.
Regression tests cover restart, blocked admission, lease expiry and resumption
after reconciliation. This follow-up is not installed on the running reviewers.

New managed Hermes tasks and direct Cursor prompts now reference a retained
SHA-256-bound evidence file instead of embedding the full history. Saved older
Hermes projections keep their original body. Tests cover 200 KB histories with
sub-4 KB transport/prompt fixtures, exact artifact bytes, replay, drift, unsafe
paths and secret rejection. Role guides describe selective evidence reading;
model choices live in the task binding rather than duplicated skill prose.
This compact-input boundary is deployed and has dispatched the first real
reviewer set; completion compatibility is not yet proven.

## Remaining completion gates

1. Finish one real issue through builder, exact-head CI, all required independent
   reviews, remediation where needed, and final human-ready disposition.
2. Finish capability/case failure isolation and safe result collection during
   pause; extend confirmed-non-start classification where still missing.
   Prove native peer-review ordering through the live adapters, not only tests.
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
