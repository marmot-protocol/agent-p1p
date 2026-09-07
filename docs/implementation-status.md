# Pip implementation status

Updated 2026-09-07. This inventory distinguishes implementation, deployment and
live proof. The target is [the lean architecture](pip-architecture-plan.md).

## Latest verified live state

Pirate was checked after the `e6f7eec4d92a26d7e141591c285fcbcdc350154c`
installation:

- Signed deployment run `34087957423` and CI `34087957390` passed, including
  the Linux service-lifecycle suite. Installation preserved the stopped ledger
  byte-for-byte and preserved the active revision-7 policy. Execution was
  quiesced for installation and then resumed; conversational Hermes was untouched.
- #993 remains the sole labeled canary, with accepted plan version 1. Its
  retained implementation commit is `05070de3ef5e151bba702f85fd4c9e510f7e0df4`.
  Attempts 7 and 8 performed useful work but failed result parsing/typing.
- The supported root retry command preserved failures and the escalation, then
  granted one additional attempt. Active policy revision 7 was restored.
  Attempt 9 completed and was accepted through the normal controller.
- Draft PR [MDK #1726](https://github.com/marmot-protocol/mdk/pull/1726) exists
  at that exact head. Its required GitHub CI passed. Full status-history parsing
  was corrected without discarding historical failures; the controller accepted
  CI and transitioned to `REVIEWING`, dispatching one native and two direct
  reviewer jobs. Cursor attempt 10 then stopped at its noninteractive workspace
  trust prompt, before a review result. The aggregate failure bound escalated
  the case. The native general review `t_6b15c1cd` subsequently completed with
  `REQUEST_CHANGES`: daemon error writers still emit unsanitized terminal
  controls. Its evidence also reproduces an overlong assigned Unix socket path.
  That result remains in Hermes, not yet accepted into the escalated case.
  Execution services/timers are now stopped and disabled; webhook ingress stays
  active. Required reviews and final readiness are not proven yet.
- #891, #1228 and #1639 are abandoned with history retained.
- Hermes remains the upstream installation; Pip has not introduced a fork.
  Conversational Hermes is separate and must not be interrupted.

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

## Local work, not yet deployed

Direct result collection is deployed separately from workflow advancement.
The paused controller can retain an exactly bound completed result without
credentials, publication, a new task, or a case transition. Resume accepts the
saved result without rerunning the provider. Tests cover restart, expired
dispatch lease after retention, malformed bindings, failures and queue traversal.
Native Hermes collection now has local coverage for the same pause/restart
behavior using the existing immutable evidence table, not another workflow
database. A broken direct queue does not prevent native collection while paused.
Native collection is not yet deployed. Active-cycle/cross-case failure isolation
and saved-policy compatibility still need work; not all pause semantics are done.
Collection also retains late completions from held/superseded work as evidence
without advancing those cases. Cursor now requests `--trust` for the assigned
workspace without adding `--force` to reviewers. These follow-up changes have
focused regression and Clippy coverage, not live reviewer completion proof.

Review-start recovery now shares the existing bounded root/offline recovery
path: `authorize-review-retry` preserves the accepted PR/head/build and creates
fresh CI observation, not a builder rerun or approval. Short scratch schema-2
paths now pass a real Unix-socket allocation test while old schema-1 paths stay
unchanged. These changes have local Rust/Clippy coverage and are awaiting their
signed release deployment. The recovery has not been applied to #993 yet.
Native result ingestion now also accepts a reviewer after peer-only
`REVIEW_RECORDED` revisions. A regression joins both real-shaped review
contracts through final-review state; intervening CI/retry generations still
fence older work. This ordering fix is not yet deployed.

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
