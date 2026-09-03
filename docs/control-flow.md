# Target control flow

This document defines how the Rust control plane converts external evidence into
one durable transition and the next bounded effect. It complements the target
architecture with operational sequencing.

## Authority rule

Only a committed ledger transition changes workflow state. GitHub objects,
Hermes task states, provider responses, and filesystem state are observations.
They must be validated before becoming accepted evidence.

Every transition follows:

```text
observe -> normalize -> validate -> deduplicate -> decide -> commit
        -> publish outbox effect -> observe effect -> reconcile
```

The decision and outbox insertion share one SQLite transaction. Performing the
external effect does not. A restart may repeat observation or effect delivery,
so every effect carries a stable idempotency key.

Hermes-native work is projected to the repository board. Direct-provider work
remains a ledger effect until the controller validates it, records an immutable
attempt, and atomically publishes a read-only inbox envelope. The separate
`pip-worker` service executes that envelope without ledger or credential
access and writes a bounded result envelope. Only a freshly authorized
controller cycle can validate, record, and ingest that result; the worker never
advances state itself.

## Case states

The initial target state vocabulary is:

| State | Meaning |
|---|---|
| `PLANNING` | A planner run is required or active. |
| `WAITING_HUMAN` | A concrete authoritative decision is required. |
| `READY_TO_BUILD` | An active plan is accepted; build dispatch is pending. |
| `BUILDING` | Builder work is active or awaiting validated completion. |
| `WAITING_CI` | A reported PR head exists but required CI is not yet acceptable. |
| `REVIEWING` | Both mandatory reviews for one exact head are pending or being joined. |
| `REMEDIATING` | A builder is addressing the current finding set. |
| `FINAL_REVIEW` | The exact-head join passed and final adjudication is pending. |
| `SHADOW_READY` | Final review passed; human review/merge is required. |
| `READY_TO_MERGE` | Final review passed and policy permits guarded merge. |
| `MERGING` | The deterministic merge transaction is active. |
| `COMPLETED` | The case reached its terminal successful disposition. |
| `BLOCKED` | An external prerequisite or recoverable condition prevents progress. |
| `ESCALATED` | A loop, disagreement, or operational bound requires a human. |
| `ABANDONED` | An authoritative decision ended the case without a merge. |
| `TAKEN_OVER` | Human ownership froze automation permanently unless explicitly returned. |

`COMPLETED`, `ABANDONED`, and `TAKEN_OVER` are terminal by default. Returning a
taken-over case requires a new explicit authorization event, not a comment that
happens to resemble approval.

## Intake sequence

1. Verify the webhook signature or fetch evidence during reconciliation.
2. Resolve numeric repository, issue, actor, and label identity.
3. Confirm current eligibility and workflow ownership.
4. Enforce global and repository pause/concurrency policy.
5. Create the permanent case and intake event idempotently.
6. Commit an outbox entry for one blocked planner projection.
7. Create/verify the Hermes task projection.
8. Revalidate authorization immediately before releasing the planner task.

Failure before a verified webhook delivery or polling observation changes
nothing. A verified delivery is retained even if the GitHub re-read fails, so
the same delivery can be retried without ambiguity. Failure after case creation
leaves a durable outbox item for reconciliation; it does not create a second
case or task.

## Planning sequence

1. Observe a completed planner task and fetch its immutable result artifact.
2. Verify case/task/profile/skill/model bindings and schema.
3. Commit the immutable planning run as `PLAN_RECORDED` plus a durable
   `PUBLISH_PLAN` effect.
4. Publish or verify the provenance-marked GitHub plan comment and record its
   content/actor evidence.
5. Only after publication, apply the typed outcome:
   - `PROCEED` -> `READY_TO_BUILD`;
   - human ambiguity -> `WAITING_HUMAN`;
   - dependency -> `BLOCKED` plus dependency metadata;
   - already-fixed/duplicate/not-reproducible -> `COMPLETED`;
   - reject/abandon -> `ABANDONED`.
6. Publish only the effect allowed by the committed state.

A trusted clarification creates a new planning run. It cannot transition
directly to building.

## Build and CI sequence

1. Allocate a case-owned branch and worktree record.
2. Revalidate plan authorization and current repository policy.
3. Dispatch one fresh builder with the revision-bound immutable evidence
   bundle, active plan, exact case-owned branch, and assigned worktree.
4. Validate and commit the returned clean local head without giving the worker
   GitHub credentials.
5. Canonicalize the assigned worktree under the controller root, require the
   policy-bound push URL, disable repository hooks/filesystem monitors, clear
   repository credential-helper/proxy/header configuration, force TLS
   verification, invoke signed askpass with only the systemd credential-file
   path, publish only the assigned branch with exact force-with-lease, verify
   the remote head, then create/update and independently verify the draft
   PR/head.
6. If CI is pending, enter `WAITING_CI` without redispatching the builder.
7. If CI fails because of the change, create a builder remediation event.
8. If CI is green and acceptable on the exact head, enter `REVIEWING` and
   publish two independent review effects in the same ledger transaction.

Infrastructure retry policy is separate from code-remediation policy. A rerun
does not erase historical evidence; repository policy determines which check
history is acceptable.

## Review and remediation sequence

For review round `R` at head `X`:

1. Dispatch both reviewers with the same immutable context and `X`.
2. Accept each result only after independent task/profile/model/head validation.
3. Wait until both results exist; never let one reviewer release remediation.
4. If both approve and the join passes, enter `FINAL_REVIEW`.
5. Otherwise create the canonical union of mandatory findings and enter
   `REMEDIATING`.
6. Dispatch one builder remediation run with that exact finding set.
7. Accept a new head `Y` only with resolution records and green required CI.
8. Invalidate all review evidence for `X`.
9. Increment the round and return to `REVIEWING` at `Y`.

The originating reviewer confirms each of its prior findings. Both reviewers
still review the entire current diff independently; confirmation alone is not
approval.

Policy checks before another round include maximum rounds, elapsed time,
repeated finding fingerprints, and repeated provider failures. Exceeding a
bound enters `ESCALATED`.

## Final review sequence

1. Rebuild the complete case bundle from immutable ledger events, runs,
   controller evidence, and findings; bind and hash it at the current revision.
2. Re-fetch and record the original issue and bounded clarification history,
   current GitHub authorization, PR head, CI, reviews, threads, ownership, and
   mergeability.
3. Commit the fresh GitHub preflight evidence and final-review dispatch intent
   atomically, then dispatch one fresh final reviewer with that exact bundle.
4. Validate and commit its immutable result.
5. Route the typed outcome:
   - `READY` -> `SHADOW_READY` or `READY_TO_MERGE` by policy;
   - return to build -> `REMEDIATING` with a final finding;
   - return to review -> `REVIEWING` on the same head;
   - return to planning -> `PLANNING` with a new plan version required;
   - human wait -> `WAITING_HUMAN`;
   - abandon -> `ABANDONED`.

Any code change after final review invalidates the final run, both reviews, and
CI evidence for the prior head.

## Merge sequence

`READY_TO_MERGE` is unreachable in MDK shadow policy. Where separately enabled:

1. Acquire a case merge lease in the ledger.
2. Re-fetch every exact-head, authorization, ownership, CI, review, thread, and
   mergeability condition.
3. Abort to the appropriate state on any mismatch.
4. Mark the controller-owned draft ready through GitHub GraphQL and re-run the
   complete gate against the non-draft PR.
5. Commit `MERGE_STARTED` and a separate durable `EXECUTE_MERGE` effect.
6. Invoke the policy-selected GitHub merge with the expected head SHA.
7. Fetch and verify merged state and merge commit.
8. Commit the merge evidence and enter `COMPLETED`.

After an ambiguous response or restart, reconciliation first observes whether
the PR is already ready or merged, then resumes without duplicating the logical
transaction.

## Pause, authorization removal, and takeover

- Global or repository pause prevents new activation but preserves running-task
  evidence for bounded collection.
- Removed issue authorization prevents every downstream activation and merge.
- The controller re-fetches exact issue/label history immediately before an
  outbox dispatch is claimed. A policy or repository identity discrepancy
  leaves the effect pending and performs no Hermes write. Authoritative label
  removal, issue closure, exclusion, or untrusted relabeling commits an
  immutable abandonment event and supersedes older pending effects in the same
  SQLite transaction.
- Human commits to or ownership changes on a Pip PR trigger takeover policy.
- Active policy names the numeric GitHub automation actor. The controller also
  derives the one allowed case branch from repository, issue, and workflow
  identity. A foreign PR author, repository, branch, disposition change, or
  protected-state head change commits `HUMAN_TOOK_OVER`; expected head movement
  is allowed only while a builder is active in `BUILDING` or `REMEDIATING`.
- A running worker may be terminated only through an identity-checked process
  lease; PID alone is insufficient.
- Passive external lookup failure retains the last durable state but cannot
  release new work.

## Recovery invariants

After restart the controller:

1. opens and migrates the ledger transactionally;
2. expires bounded leases using injected time;
3. replays unsatisfied outbox entries idempotently;
4. reconciles active Hermes projections, direct-job leases, and GitHub objects;
5. accepts completed results at most once;
6. releases no task based only on cached external state; and
7. emits a durable discrepancy event for foreign or conflicting state.

Recovery never reconstructs workflow state solely from task titles, parent
summaries, or mutable issue prose.
