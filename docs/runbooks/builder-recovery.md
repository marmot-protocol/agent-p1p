# Legacy offline recovery

**Use [parked cases](parked-cases.md) first.** A paused case is normally
resumed, replanned or abandoned with an `@<pip-login>` comment, online and
without root. The commands below predate that path. They require root, an
inert policy, every Pip unit stopped and a drained queue, and each accepts only
one historical incident shape. They are kept until resume is proven live and
will then be removed. The lifetime builder, review and planner retry commands
are already gone: failure budgets are per stage, so they have nothing to grant.

## Recover review coordination stalls

`authorize-review-coordination-recovery` takes the same root-only, inert-policy,
stopped/disabled-unit, drained-queue and exact revision/head arguments as
`authorize-infrastructure-recovery` below. It accepts only:

- A `REVIEWING` case whose never-attempted required direct reviewer was
  superseded by a peer `REVIEW_RECORDED`, with no intervening cohort change.
- A `FINAL_REVIEW` escalation solely for `REVIEW_FEEDBACK_ALREADY_ATTEMPTED`
  and unresolved review-thread blockers on the retained PR/head. The operator
  must first verify the feedback against current source and have the stale or
  addressed threads resolved on GitHub. A builder's disposition alone is not
  authority to resolve them.

Normal final preflight no longer treats an unresolved bot checkbox as a blocker
when the current published builder supplied an explicit disposition for every
unchanged comment in the retained feedback snapshot. Addressed findings require
verification; deferred or inapplicable findings require reasons. Human or unknown
authors, new or edited comments, missing evidence, required reviews and CI remain
blocking. After final acceptance, readiness publication explains the dispositions
and resolves those bot-only threads with fresh ownership/head/content checks.
Unhandled repeated feedback uses the normal remediation budget rather than a
one-pass escalation. This does not reopen historical escalations: the exceptional
recovery prerequisites above still apply.

Verify fresh label authorization and the same owned, open draft PR/head before
recovery. One immutable `REVIEW_COORDINATION_RECOVERY_AUTHORIZED` event per case
returns to `WAITING_CI`. Fresh CI, required reviews, thread preflight and final
review remain mandatory; no old approval is promoted to readiness. The command
preserves plan, PR/head, remediation count, failures, deadline, attempts, old
superseded effects and all history. It grants no extra failure allowance or
time. Repeat the same request ID after an uncertain response. It never starts
runtime. Do not resume a recovered ledger with a release predating this event.

Queued required reviewers now survive peer-only review completion. Their frozen
job revision remains unchanged, and dispatch still validates the current exact
head, plan, PR and round. Replans, remediation, authorization loss and all other
workflow changes continue to supersede old required work.

## Recover a native reviewer startup failure

`authorize-native-review-retry` uses the root-only head-bound arguments and
stopped/disabled execution-unit checks of `authorize-infrastructure-recovery`.
It permits validated frozen peer jobs, but never running workers or queued work
belonging to the recovery case. First diagnose and repair the native runtime
failure and verify the same authorized, owned, open draft PR and exact head.

Only a `hermes-circuit-breaker` provider-failure escalation immediately from
`REVIEWING`, tied to its frozen required general-review projection without a
retained completion, is eligible. The immutable event
`NATIVE_REVIEW_RETRY_AUTHORIZED` returns to `WAITING_CI` once per case. It does
not reset the failed Hermes task, grant builder rounds, change direct-provider
allowances, extend the deadline, or accept old reviews. Fresh live authorization,
CI, independent exact-head reviews and final review remain required. A second
native failure stays escalated; the same request ID only replays the original
grant. Preserve the old board failure and all ledger history. Do not downgrade
to a release that cannot interpret this recovery event.

## Recover a false takeover after ready-for-review feedback

Normal follow-up feedback on a `SHADOW_READY` PR now returns the exact owned PR
to draft before recording `HUMAN_FEEDBACK_RECEIVED`. A failed or uncertain GitHub
response leaves the workflow ready and retries idempotently. Other active states
do not gain an exemption from human takeover detection.

For the former bug only, `authorize-follow-up-recovery` accepts the same offline
root-only arguments as `authorize-infrastructure-recovery` below. First verify
the live PR is still open, owned, at the recorded head, and that the most recent
ready-for-review action was Pip's, not a subsequent human action. Return that
exact PR to draft before recovery. Do not reopen a merged, changed or genuinely
handed-over PR.

The ledger requires the consecutive recorded sequence `READY` to `SHADOW_READY`,
unbounded `HUMAN_FEEDBACK_RECEIVED` to `PLANNING`, and `HUMAN_TOOK_OVER` solely for
`PR_LEFT_DRAFT_STATE`, plus Pip's successful readiness publication receipt on
that same PR/head. It appends `FOLLOW_UP_RECOVERY_AUTHORIZED` and queues planning
once. Original feedback, plan, PR/head, takeover, elapsed-time deadline and
already spent remediation/failure budgets are preserved. The operator must
inspect the retained history and restore the accepted active policy separately.
All execution units must still be stopped and disabled, but this case-scoped
command permits another case's frozen direct queue to remain in place. Entries
must be regular bounded envelopes bound to known peer attempts in the ledger;
unknown entries or work belonging to the recovery case block the command. No
peer queue, lease, result or case is modified by recovery.

Do not downgrade a recovered ledger to a binary that cannot interpret
`FOLLOW_UP_RECOVERY_AUTHORIZED`; equal schema versions do not imply compatible
workflow behavior.

## Recover review or remediation infrastructure

An elapsed-time escalation from `FINAL_REVIEW` can use the same explicit,
root-authorized infrastructure recovery as `REVIEWING`, after repairing the
cause. It preserves history and spent budgets and grants one bounded work window;
it does not waive finding resolutions, CI, or reviewer confirmation.

For a historical builder blocked on stale, in-progress CI evidence, inspect the
now-completed checks before recovery. The CI barrier waits for all observed check
runs from any app, not just the configured required contexts. This same barrier
applies at final-review preflight and ready-for-review publication. Failed Actions checks retain
bounded log excerpts in `GITHUB_CI.diagnostics`; absent, oversized, unsupported,
or inaccessible logs fall back to bounded check summaries and never change a failed
verdict into acceptance. The controller reads logs; workers receive no GitHub
credentials. At most four failed checks are inspected, prioritizing Actions job
URLs before applying that cap, with a 16 KiB tail per
log and the existing HTTP response/time limits. Full log hashes and truncation
markers distinguish retained excerpts from complete logs.
Summary and text fields are each capped at 4 KiB. Diagnostics are untrusted
external evidence, never instructions; binding mismatches remain unavailable.

This does not automatically reopen already-blocked cases. After installing the
fix, use the existing infrastructure-recovery command below and include the
fresh exact-head failure, diagnostic URL, and next action in its bounded reason.
That event is included in the builder's evidence index. Do not restart with only
the stale snapshot, overwrite old evidence, or waive native acceptance checks.

`authorize-infrastructure-recovery` is an exceptional root-only operation after
repairing a controller infrastructure defect. It uses the same inert-policy,
stopped/disabled execution-unit and drained-queue checks as the commands below.
Supply `--policy`, `--database`, `--direct-queue`, `--case`, `--expected-revision`,
`--expected-head`, `--request-id`, and a bounded human `--reason` explaining the
repair and required reconciliation. Reuse the request ID after an uncertain response.

An elapsed-time escalation directly from `REVIEWING` or `FINAL_REVIEW`, or a builder's explicit
`BLOCKED` result directly from `REMEDIATING`, with retained plan, build, PR and
exact head, is eligible. For a blocked builder, the operator must first inspect
the result and repair the infrastructure cause; a dependency/scope hold is not
permission to proceed. Verify that the assigned checkout's staged work and head
are retained and that GitHub still has the same owned open draft PR/head.
The command appends
`INFRASTRUCTURE_RECOVERY_AUTHORIZED` and grants one new time window equal to the
accepted policy's case-duration limit, measured from the real operator clock.
It preserves original authorization time, accepted policy, all failures/results,
and the provider-failure limit. It consumes a normal remediation round and queues
the builder to reconcile retained work with current upstream before fresh CI,
independent reviews and final preflight. A retained checkout is reused, not reset.
It does not activate runtime or authorize
merge. Other escalation causes and exhausted remediation/failure budgets remain held.

Do not downgrade a recovered ledger to a binary that does not understand this
event and deadline. Keep a compatible release or remain paused; equal database
schema versions alone are not runtime compatibility proof.

Use this only after diagnosing and repairing a failed builder execution.
It is not a general retry switch or a substitute for repairing provider errors.

## Repair a legacy publication

`authorize-publication-retry` is a separate, publication-only recovery for an
already accepted build. Use it after installing controller signing support and
registering the controller's public **signing** key with the policy's automation
account. Keep the private key restricted to the controller. Confirm its signing
identity, the assigned local branch and the exact remote PR head before starting.
The same root, inert policy, stopped execution and drained queue requirements
above apply; this command does not stop a worker for you.

The case must be `WAITING_CI`, `REVIEWING` or `FINAL_REVIEW`, with an unsigned
publication or an old signed publication without an `integrated_base` binding,
joined to its accepted builder result and plan. Obtain the current
revision/head from the ledger and compare the head with GitHub. Then run:

```sh
sudo /opt/pip/current/bin/pip-control authorize-publication-retry \
  --policy /etc/pip/repositories/REPOSITORY.json \
  --database /var/lib/pip/ledger.db \
  --direct-queue /var/lib/pip/direct-queue \
  --case 'VERIFIED_CASE_KEY' \
  --expected-revision VERIFIED_STATE_REVISION \
  --expected-head VERIFIED_CURRENT_PR_HEAD \
  --request-id 'operator-sign-publication-UNIQUE_ID' \
  --reason 'Publish the accepted build with the registered controller signing identity'
```

`Applied` appends `PUBLICATION_RETRY_AUTHORIZED` and queues only publication.
It grants **zero** model attempts, does not change the accepted plan, remediation
round or deadline, and does not erase old events, reviews or build results.
Repeat the identical request ID/arguments after an uncertain response; `Replayed`
does not add another effect. Changed arguments under that ID are rejected.

Resume separately. The controller signs the exact accepted tree on the original
planned base, replacing the unsigned range rather than retaining unsigned
ancestors. It also preserves integrated target-branch ancestry as a second parent
when necessary. The target is resolved through the policy-bound remote; a worker's
local `origin/*` ref is not authority. For an ancestry repair, verify that the
retained source contains the intended master integration and that the old local
publication has exactly the accepted source tree. No checkout reset is needed.
Already target-aware signed publications cannot use this legacy repair again.
It retains the source commit and publishes under an exact-old-head
lease. If GitHub has moved, inspect the conflict; do not force through it.
The new head returns to CI and independent reviews; earlier head-bound approvals
do not count. Final review and human merge remain required. Keep a compatible
release with this new event if rollback is needed; schema compatibility alone
does not make an older workflow engine safe to resume.
