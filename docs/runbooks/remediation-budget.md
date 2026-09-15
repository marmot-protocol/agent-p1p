# Remediation budget

The MDK target policy now permits **ten remediation rounds**, through
`max_remediation_rounds: 10`. The limit is repository policy, not a hard-coded
case identity. Historical activation policies and accepted ledger policies are
not rewritten.

## What currently consumes a round

The authoritative increment is in `pip-controller/src/ledger.rs`:

| Event | Remediation charge |
| --- | --- |
| Failed CI returning `WAITING_CI` to `REMEDIATING` | One |
| Required review requesting changes and entering `REMEDIATING` | One for the combined correction pass, not one per finding or reviewer |
| Final review returning to build, including relevant suggestions or mergeability corrections | One when it enters `REMEDIATING` |
| Accepted human feedback returning the workflow to planning | One |
| Exceptional `INFRASTRUCTURE_RECOVERY_AUTHORIZED` returning to remediation | One under the current implementation; not yet infrastructure-exempt |
| Operator `REMEDIATION_BUDGET_EXTENDED` | Zero; preserves spent rounds and requests fresh CI observation |

An initial build is round zero. A round is charged when the corrective pass is
authorized, not when its worker finishes. At ten, successful CI/reviews may still
finish the PR; another correction is held rather than dispatching round eleven.

## What does not consume another remediation round

- Polling, duplicate webhook deliveries, and replay of an accepted event.
- Waiting for CI, review completion, capacity, or a human response.
- Publishing a plan, build or review; retrying publication of the same work.
- Multiple findings in the same correction pass, or commands/tests run within
  that builder attempt.
- Normal review/final-review dispatch and successful transitions.
- A provider failure or an infrastructure observation by itself. These have
  separate failure/time limits; this does **not** imply free automatic retries.

A runtime failure after a corrective pass was authorized does not refund the
already charged round. Returning from a hold through the exceptional
infrastructure recovery command currently charges a new round. Do not claim
infrastructure failures are fully exempt until that behavior is changed.

GitHub check failure is not presently classified as product defect versus runner
outage. If it produces `CI_FAILED` and starts a corrective pass, it charges a
round. A broken/flaky test therefore counts when it needs code work; assertion
failures must not be reclassified as infrastructure merely to bypass the limit.

## Separate limits remain

The MDK policy still has a 24-hour elapsed-case limit, provider failure limit of
three (with recorded case-specific recovery allowances where present), repeated
finding fingerprint limit of two, and per-role runtime/attempt limits. Ten rounds
does not guarantee ten executions: these independent guards can stop work first.
Waiting/outage time does not add a remediation round, but currently still counts
toward elapsed age. New commits do not reset these budgets.

## Rollout and existing cases

Install a fresh policy revision rather than editing an accepted revision's
payload. Preserve all other live settings, including model bindings, concurrency,
label/actor authorization and human-only merge. Do not install the paused target
template verbatim over a running host policy.

Existing cases retain their accepted revision and spent rounds. A changed
remediation budget is intentionally not treated as a capacity-only rollout by
`RepositoryPolicy::execution_policy_for`. In particular, changing the current
policy to ten **does not resume or migrate an escalated three-round case**.
It can also fence old-policy execution/conversation routing until compatibility
or an explicit migration is provided. Do not switch the live policy while such
cases still need service merely to advertise the new limit.

`authorize-remediation-extension` is the supported offline transition for a case
whose latest event is `CI_FAILED` from `WAITING_CI` to `ESCALATED` at its old
remediation ceiling. It requires an open, still-authorized owned draft PR at the
recorded head (operator verification), root, stopped/disabled execution units,
an empty direct queue, no running attempts or leased case work, and a fresh
policy revision that differs from the accepted policy **only** in revision and
an increased `max_remediation_rounds`. Supply that new policy in paused form;
its original active flags are restored in the recorded accepted policy.

The operation atomically records `REMEDIATION_BUDGET_EXTENDED` and the new policy,
preserves plan, PR/head, old rounds, results and authority, and returns the case
to `WAITING_CI`. It does not dispatch a builder directly. Normal live
authorization and exact-head CI observation determine whether another correction
is needed. The extension itself costs zero rounds; a subsequent corrective pass
costs one. Expired cases and other escalation causes are rejected. No deadline,
provider allowance, model, scope or merge authority is changed.

Use the same root-only preconditions and options as
[`authorize-infrastructure-recovery`](builder-recovery.md), replacing the command
name with `authorize-remediation-extension`. The `--policy` file must contain the
new revision/ceiling. Reuse the request ID after uncertain responses; replay does
not extend the budget again. Verify the exact new active policy before resuming.
Do not edit policies/cases in SQLite, reset the counter, remove/re-add the label,
or use infrastructure recovery for an unrelated CI escalation.

Do not downgrade a migrated ledger to a release that cannot interpret this event.
Equal schema versions do not imply behavioral compatibility.

## Follow-up design, not implemented by this configuration change

Separate bounded infrastructure recovery from code-remediation accounting;
classify confirmed pre-execution failures without spending a work allowance.
Add evidence-based no-progress detection and planner reassessment before human
escalation. Keep an independent total resource limit so changing the failure
message or creating a commit cannot extend execution indefinitely. Extension for
other escalation causes remains outside the current operation's scope.
