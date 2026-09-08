# Pip architecture

Status: approved lean target, 2026-09-06; implementation and live proof in progress.
This is the canonical architecture required by `AGENTS.md`. It describes the
target, not a claim that the live pipeline is complete.

## Purpose

Turn an explicitly authorized repository issue into a draft PR with a validated
plan, implementation, independent reviews, and a final merge-readiness decision.
Pip is a small Rust workflow coordinator around unmodified Hermes and Cursor,
not another general-purpose agent platform.

The MDK pilot remains shadow/human-merge-only. No model or worker may merge.
Repository and issue identities, models, actors, limits, and branches come from
validated policy and live evidence, never constants in the generic engine.

## Ownership

| Component | Authority |
|---|---|
| GitHub | Issues, labels and their actors, branches, PR heads, reviews, CI and merge status |
| Rust ledger | Accepted workflow decisions, job definitions/results, findings and intended GitHub effects |
| Hermes | Native task dispatch, claims, run lifecycle and operator-facing repository boards |
| Cursor adapter | Execute one assigned job and return its result; no independent workflow decisions |
| Human | Scope decisions, takeover, exceptional recovery and merge |

One SQLite database remains authoritative for Pip. Hermes task completion is an
execution observation, not approval to advance the workflow. Queue transport
files are messages, not a second workflow database.

There is one logical job contract with two narrow execution adapters. Do not
force Cursor into an unsupported Hermes provider or maintain a Hermes fork.
Native external-CLI lanes are not assumed to be a supported integration merely
because Hermes has an internal spawn callback. Any adapter must prove its real
lifecycle boundary before use.

## Workflow

1. Re-read an open, authorized issue and its trusted label event.
2. Create a stable case and ask the planner to validate the issue, root cause,
   scope, dependencies and test plan against current source.
3. Accept a versioned plan or record an explicit human/terminal disposition.
4. Ask the builder to implement that plan, test it and produce a local commit.
5. The controller signs the accepted build tree, retains its source commit and
   records the source-to-published commit binding, then publishes the signed
   commit to the assigned branch and creates/updates one draft PR.
6. Observe required CI for that exact head.
7. Run every configured required reviewer independently on the same head.
8. If changes are needed, combine blocking findings, remediate, and repeat CI
   and the required review set on the new head.
   Builders also assess nonblocking suggestions and record addressed/deferred
   decisions with reasons. The final reviewer assesses suggestions when no
   remediation occurred, returning worthwhile in-scope changes to the existing
   build loop. Suggestions alone do not force a new round or expand authority.
9. Run a fresh final review of the issue, plan, implementation and review history.
10. Revalidate authorization, exact-head CI/reviews and final acceptance; mark the
    draft PR ready for review and publish a human-held readiness recommendation.
    A person reviews and merges. Promotion and notification retry idempotently;
    leaving draft after accepted final review is not itself a human takeover.

This is a dynamic bounded loop, not a pre-created multi-round DAG. Rust creates
only currently authorized work; no synthetic activation-gate cards. Hermes
dependencies may organize execution, but never bypass result acceptance.

Planner outcomes distinguish proceed, already fixed, duplicate, not reproducible,
different scope, external dependency, human clarification, blocked and abandoned.
Final review can recommend ready, more implementation/review/planning, human
input, blocked or abandoned. Neither role guesses missing product intent.

## Jobs and evidence

### GitHub conversation lane

Opt-in mentions and human feedback use the existing webhook/queue/publication
boundaries, with a separate durable inbox in the same Rust ledger. They are not
new issue authorization. A native conversation task answers a question or
recommends a bounded planning follow-up; only the controller may hand feedback to
an existing, freshly authorized case at a safe worker boundary. This lane never
approves CI, review heads, sensitive scope or merges. See
[GitHub conversations](github-conversations.md) for behavior and rollout gates.

### Case jobs

Every job freezes its case/round, role/reviewer identity, exact provider/model,
reasoning settings, skills version, workspace, input references and execution
limit before dispatch. No silent model substitution is permitted.

Recovery reads the saved definition rather than rebuilding it from the current
release or profile defaults. Existing jobs retain their original inputs across
upgrades. New jobs may use newly approved settings. Immediately restrictive
controls (pause, revocation, takeover) apply to old and new work.

Each result is schema-validated, bound to the authorized job and accepted at most
once. Late results cannot advance a superseded round or revoked case. Record
what model identity can actually be observed; do not claim provider attestation
that the provider does not supply.

Keep accepted results, relevant GitHub observations and decision history durable.
Give workers compact role-specific evidence, including the accepted plan and
applicable findings. Keep full history available through immutable artifacts;
do not copy the entire growing ledger into every prompt. Bound individual
payloads and preserve provenance without making unrelated formatting or release
changes invalidate a job.

The ledger transaction records a decision and its intended effects together.
External publication reconciles stable ownership markers and exact heads after
timeouts/restarts. Do not promise exactly-once physical execution across an
uncooperative external queue. Prevent overlapping workspace writers, fence stale
results, and make publication idempotent. Uncertain execution must be inspected
before a retry; ordinary confirmed non-starts must have a supported recovery path.

## Reviews and readiness

Reviewer instances are policy records, not compiled role/model combinations:
stable ID, semantic lane, exact model/provider, executor and review mode.

- Required reviewers block readiness and contribute mandatory findings.
- Advisory/shadow reviewers are comparison observations only. Their failures,
  latency or absence must not consume the main workflow's failure allowance or
  delay required work.

The initial semantic lanes remain general and security/performance. Additional
models do not require new engine code. Lane publication uses the configured
distinct GitHub App identities, separate from the PR author.

Readiness requires current authorization and ownership, the accepted build,
green required CI, all required reviews and final review bound to the same
current PR head, no unresolved mandatory findings, no blocking GitHub reviews
or threads, and clean mergeability. A new head invalidates earlier head-bound
approvals. Applicable findings require resolution confirmation by their origin.

Controllers publish plans, draft PRs, lane reviews and readiness comments using
stable markers. Workers do not receive GitHub publication credentials.
GitHub text is for humans: concise outcomes, scope, actionable findings, reported
checks and limitations. Do not embed serialized results, internal paths or hash
inventories in visible comments. Keep structured evidence in Pip; when an export
is useful to a person, provide a separately downloadable file rather than a code
block. Small ownership markers may remain hidden. Publication formatting must
not change accepted results, review votes or exact-head bindings.
PR titles describe the change. Descriptions briefly explain the problem and
implemented solution, link the accepted published plan, and include a closing
`Fixes #N` reference. Reviewer-role identification belongs in hidden metadata,
not a visible control-language footer.
The dedicated commit-signing key is controller-only as well. Signing preserves
the accepted tree exactly and uses a validated parent and automation identity;
it never edits the accepted worker result to substitute a new SHA. Retain the
original source commit before replacing workspace refs or reclaiming storage.
CI, reviews and final readiness bind the published signed SHA, not its unsigned
source SHA. Replacing an existing unsigned publication requires an audited
recovery decision and fresh head-bound CI and reviews.
Automatic merge is deferred; it is not part of the lean runtime's required
execution path and cannot be enabled accidentally by a generic configuration.

## Failure and recovery

Infrastructure unavailability is not a failed attempt to solve an issue.
Distinguish unavailable runtime/authentication, transport failures, invalid
results, task failure, and human scope blockers. Preserve observations without
spending a work-attempt allowance before useful work starts.

Infrastructure retries require bounded backoff and a successful readiness probe;
no busy retries or unlimited provider spending. Bound actual runs and remediation
separately. Paused/outage time must not silently burn an issue's work budget.
An explicit overall age limit may still require human attention.

Failures are local to the affected job or capability. Mint reviewer credentials
when publishing reviews, not before planning or ingesting every result. Keep
completed-result ingestion and safe housekeeping available during a dispatch
pause. One failed cleanup or publication must not prevent unrelated progress.

Provide ordinary status, pause/resume and explained retry/recovery operations.
Exceptional recovery appends history; it does not delete failures, alter accepted
results or require case-specific shell scripts. Recovery must prove a previous
worker is stopped before another writer starts.

## Intake and configuration

Keep signed webhook intake for low latency and bounded polling for missed events.
Retain the isolated loopback receiver, delivery-ID deduplication, bounded durable
spool, authenticated payloads and live issue revalidation. Public ingress never
receives repository credentials, provider credentials or ledger access.

Eligibility checks current label, trusted numeric actor, open issue/repository
identity, exclusions, pause and capacity limits. Label removal and takeover stop
new work and downstream publication; do not promise instant termination of an
already executing model. Replayed deliveries do not create duplicate cases.

A fresh trusted label after withdrawal may reauthorize an abandoned pre-PR
case through the controller poll once old jobs are terminal or removed and no
direct attempt is running. Append a new planning generation under current policy;
preserve prior plans, failures, effects and workspace-retirement records. Restart
only the elapsed-time window, not failure allowances. Capacity and pause rules
still apply. Completed work, human takeovers, other abandonment decisions and
cases with a PR require explicit recovery rather than label-driven restart.
Webhook intake records the delivery but cannot supply the execution-quiescence
check needed to restart an existing case.

Separate operational switches from immutable accepted work definitions.
Configuration changes must not require rewriting case history. Use supported
structured Hermes interfaces where available and narrowly scoped semantic
compatibility checks otherwise; human-readable formatting is not a contract.

## Runtime and storage

Keep controller secrets/state separate from code-executing workers. Workers may
run repository tests and provider runtimes without access to the ledger or
GitHub credentials. Scope systemd protections to actual capabilities: Cursor
and Node tests require JIT memory; this does not justify weakening controller
or ingress services. Retain no-new-privileges and restricted writable paths.

Use one supported case workspace layout, one assigned branch and controlled
publication. Disable worker-controlled Git hooks, credential helpers, URL
rewrites and configuration injection before credential-bearing operations.
Never share a private credential-bearing Git configuration with a worker.

Keep workspaces/build output on the managed storage volume with a free-space
reserve. Disposable build caches have separate retention from accepted artifacts.
Cleanup must not delete active work or unpreserved commits and must not block
unrelated workflow progress. Legacy workspace layouts exist only for explicit
migration/retirement, not parallel permanent execution paths.

## Packaging and code organization

Keep one Rust executable with a pure domain core, a durable store and narrow
GitHub/Hermes/Cursor adapters. Modules or crates are boundaries only when they
enforce a real responsibility; do not add abstractions for hypothetical engines.

Keep the working signed release verification and atomic install/backup boundary.
Normal deployment must preserve jobs and operational state without custom
activation scripts or manual digest juggling. Test the actual installed worker
environment, not only separate host-shell probes. Hermes stays upstream and is
upgraded through a small real-interface compatibility suite.

Retire the Python runtime after the Rust end-to-end cutover proof. Preserve the
small useful parity fixtures and historical source in Git, not two maintained
implementations. Remove unused alternate execution paths after migrating their
useful tests onto production paths.

## Verification and completion

Use strict TDD for executable changes. Preserve tests for meaningful boundaries:
exact-head joins, independent reviewers, revocation, stale results, publication
replay, bounded failures and safe workspace lifecycle. Do not retain tests solely
to preserve obsolete architecture.

The proof ladder is: pure/adapter tests; transaction/restart tests; actual
service-identity execution tests; verified release installation; one real issue
through planner, builder, CI, independent reviews and final readiness.

The lean migration is complete only when:
- one live issue produces a PR ready for human review and merge;
- required reviews and CI are verified on its exact current head;
- the agreed failure isolation, saved-job recovery and duplicate-path removal
  are implemented and tested;
- runtime/history are preserved and no Hermes fork is required;
- obsolete code/docs are retired rather than hidden behind new abstractions;
- webhook intake remains functional and autonomous merge remains disabled.

Local tests, installed behavior and live provider/PR evidence are distinct.
See dated evidence under `docs/evidence/` for historical observations; those
snapshots are not current deployment status.
