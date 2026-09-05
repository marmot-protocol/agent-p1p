# Gate-free Hermes dispatch assessment

Status: gate-free Rust dispatch implemented and tested locally; an isolated
real-Hermes CLI/scheduler recovery test passed on Pirate. Not installed or
activated in the production Pip runtime. A real planner run remains unproven.

## Recommendation

Keep unmodified Hermes as an execution queue. Remove synthetic activation-gate
cards from Pip's future dispatch path. The Rust ledger authorizes each role's
work before the controller creates an ordinary, parentless, assigned Hermes
task. Completion of a Hermes card alone never authorizes the next role.

This follows the target architecture's existing ledger-authority rule. It is
not a proposal to replace Hermes, fork it, or relax result/authorization checks.

## Verified boundary

The following observations describe the pre-refactor failure and motivated the
changes below.

Pirate runs upstream Hermes commit
`29112bef099274229cadff79cdff7bf7b99c4b77`, without local modifications made by
this investigation. Its `create_task` and `recompute_ready` behavior explains
the failed canary: `initial_status=blocked` creates no sticky-block event, so a
parentless gate is promoted to `ready`. The Rust gate adapter rejects it.

An isolated probe using that installed Hermes implementation, a temporary
Hermes home and database, and no model or production board produced:

```json
{"initial":"ready","claimed":"running","completed":true,"terminal":"done","retry_running_same_id":true,"retry_done_same_id":true,"task_count":1}
```

The probe exercised normal creation, scheduler recomputation, claim, completion,
and repeated creation using one idempotency key. It did not execute a planner,
invoke a provider, or prove the complete Pip-to-Hermes integration.

Source inspection also found:

- Hermes's idempotency lookup excludes archived tasks. The lookup occurs
  before its insertion transaction; concurrent creators can make duplicates.
  A caller cannot claim exactly-once execution from this key alone.
- Hermes uses `running`; Pip's existing projector accepts `in_progress` but
  not `running`. Fast worker starts would expose this next.
- The current projector compares identity, title, assignee, creator, and a
  projection key, but not the complete body, workspace, provider/model or
  dependency set. Recovery needs stronger immutable-intent checks.
- Pip persists task projections when acknowledging dispatch, after external
  task creation. A task can run before that acknowledgment, with or without
  synthetic gates. The outbox is durable, but the exact resolved task intent
  should also be frozen before any enqueue operation.

Relevant Pip code: `crates/pip-control/src/dispatch.rs`,
`crates/pip-controller/src/scheduling.rs`,
`crates/pip-hermes/src/projection.rs`, `crates/pip-hermes/src/lib.rs`, and
`crates/pip-store/src/lib.rs` (`complete_dispatch_outputs`,
`unconsumed_task_projections`, `claim_effect_matching`).

## Implemented dispatch protocol

1. In a ledger transaction, bind the authorization/case revision and each
   immutable worker intent: evidence, role/reviewer, exact profile/model,
   skills revision, workspace, and stable dispatch key. No queue writes yet.
2. Reserve at most one create attempt per intent in an immediate SQLite
   transaction. Unlike a time-limited lock, this reservation is never recycled.
   Even if a previous create subprocess remains alive after timeout, no second
   caller is granted creation. This replaces the proposed board-lock approach
   with a stricter fail-closed protocol.
3. Recheck current authorization and policy before enqueue. Search all task
   states, including archived, for the stable identity. Adopt an exact match;
   stop on duplicates, conflicting content, or an ambiguous prior create.
4. Create a normal parentless worker task using the supported CLI and frozen
   intent. Do not pre-create future workflow stages or depend on `blocked`
   remaining sticky. The task may immediately start or finish.
5. Record the external task identity and acknowledgment. After a crash between
   steps 4 and 5, reconcile the existing card instead of creating fresh work.
   A deleted/unrecoverable card must not cause blind redispatch.
6. Ingest results only for a known authorized intent, validating exact role,
   model, evidence/case revision, and PR head where applicable. Deduplicate
   result ingestion in the ledger. Only ledger transitions schedule successors.

For reviewer fan-out, each task has a distinct frozen intent. Partial enqueue
must be recoverable. A full approved review set, not a board dependency graph,
controls progression to final review.

Authorization removal cannot be atomic with an external GitHub read and Hermes
enqueue. Document the boundary: stop future dispatch, request supported task
cancellation where possible, reject stale results and prevent downstream
   publication. Do not promise instant interruption of an already-running model.

### Executed compatibility test

`crates/pip-control/tests/dispatch_cycle.rs` includes the opt-in test
`real_hermes_cli_and_dispatcher_recover_a_lost_create_reply`. On Pirate it passed
against the unchanged installed Hermes commit above. The test:

- creates a temporary home, board, profile directory, workspace, and ledger;
- uses the Rust dispatch path and real `hermes kanban create/list/show` CLI;
- intentionally loses the successful creation reply;
- invokes upstream `dispatch_once` with its supported `spawn_fn` test seam;
- verifies that the callback sees the claimed task in `running`, in the exact
  Pip-assigned directory, and completes it with no model or network invocation;
- reclaims the outbox lease and adopts that same `done` card; and
- proves one card, one projection, no gate, and a delivered outbox effect.

The test root is created by `tempfile` and removed when the test completes. No
production task, profile, authentication file, or service is used. The callback
is not a real planner, result contract, or provider execution proof.

Run on a host with stock Hermes installed:

```sh
PIP_TEST_HERMES=/usr/local/bin/hermes \
PIP_TEST_HERMES_PYTHON=/usr/local/lib/hermes-agent/venv/bin/python \
cargo test --locked -p pip-control --test dispatch_cycle \
  real_hermes_cli_and_dispatcher_recover_a_lost_create_reply -- --ignored --nocapture
```

The refactor also fixes the real `show --json` envelope and recognizes Hermes's
`running` status. Projection checks compare complete bodies and the CLI's
returned model/provider, skills, workspace, retry-limit, and priority fields;
`show` must confirm an empty parent set. The CLI does not expose every creation
option (notably its runtime limit), so this is not full backend attestation.

Hermes receives `dir:` for Pip's existing worktree. Upstream `worktree:` may
allocate `.worktrees/<task-id>` on another branch, which is not Pip's assignment.

Schema 8 adds immutable `dispatch_batches` and `dispatch_create_attempts`.
There is no destructive reset/backfill of old cases, task projections, or
attempt history. A deployment must still use the installer's backup/migration
boundary; the production ledger remains schema 7 until an approved deployment.

## Required proof before live activation

The unit/recovery tests now cover reservation races, lease expiry/restart,
frozen fan-out membership, stale/revoked/delivered claims, full projection drift,
lost replies, fast running/done workers, missing/archived tasks, and unexpected
dependencies. The real queue/scheduler proof above has also passed. Keep the
following list as the broader activation checklist, not a claim that a full
planner or PR workflow already succeeded:

- A single creator under concurrent cycles, lease expiry, and subprocess timeout.
- Crash before create, after create but before reply, and before ledger ack.
- Task already running/done when the controller first reads it.
- Archived, deleted, duplicate, and mismatched task recovery without redispatch.
- Label revocation before/after enqueue and stale result rejection.
- Partial reviewer fan-out and exact-head result acceptance.
- Real unmodified Hermes scheduler in an isolated home, with a deterministic
  no-network test worker; exercise the actual CLI, not only fake command output.
- Then one actual planner run using the configured model in an isolated test
  board/workspace. This is a separate provider-dependent execution gate.

Only after these pass should a live canary be reconsidered. The saved MDK #891
case is ABANDONED at revision 2, with zero ledger runs/projections and one orphan
ready gate. Preserve that evidence. Decide explicit reconciliation/retirement
and case-retry semantics before touching it; the old empty-board activation
script is no longer applicable. Do not reset the ledger to make a retry pass.

## Scope and upgrade posture

This is a bounded Pip refactor across dispatch intents, queue projection,
recovery/result ingestion, and compatibility tests—not a one-line gate removal.
The workflow engine, GitHub intake, direct Cursor transport, review policies,
release installer, and merge mode can largely remain unchanged. Hermes stays
upstream. Upgrade confidence comes from rerunning the real boundary tests
against a candidate Hermes version, not assuming its CLI flags imply stable
scheduler semantics.

## Validation and host disposition

Final checks passed:

- `cargo test --workspace --quiet` (the explicit real-Hermes test is ignored in
  the default suite and was run separately on Pirate);
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --all --check` and `git diff --check`;
- `bash scripts/test-systemd-lifecycle.sh`: clean install, idempotent reinstall,
  upgrade, intentional failed-upgrade rollback, restart recovery, and disabled
  timer preservation in a disposable container; and
- the isolated stock-Hermes CLI/scheduler test described above.

The store suite has 40 tests, including global projection-identity create
reservation uniqueness across different source effects. The dispatch suite
has nine default tests plus the opt-in real-Hermes test. Mixed Hermes/direct
review fan-out is recovered after failure before acknowledgment; no direct job
is exposed early and the existing Hermes reviewer is not recreated.

Pirate was rechecked after the isolated probes: upstream Hermes has no working
tree changes, production policy still hashes to
`9750f3bd6d3d3cd0216ae0ab19858719de60f371d694899e49169a1231c34196`,
and production remains schema 7 with the same abandoned case, two events, two
evidence records, zero runs/projections, and no pending outbox work. The
temporary 1.2 GiB test-source/build directory was removed after validation;
no production deployment or canary cleanup was performed.

Next: the real planner/result-contract proof in isolation, not another live
label attempt. A deployment candidate and explicit abandoned-case recovery
decision are still required before renewed live activation.
