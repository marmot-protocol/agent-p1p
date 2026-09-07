# Exhausted builder recovery

Use this only after diagnosing and repairing a failed builder execution.
It is not a general retry switch or a substitute for repairing provider errors.

## Preconditions

- Install a release containing `authorize-builder-retry`, while paused.
- Keep the installed repository policy inert: intake disabled and paused,
  dispatch disabled. Stop and disable all Pip execution timers/services and the
  service-owned Hermes gateway. The conversational gateway and webhook ingress
  are separate and need not be stopped.
- Confirm no provider process remains. Drain/reconcile the direct queue's
  `inbox` and `results`; preserve history and artifacts, rather than deleting
  evidence to pass this check.
- Verify the repaired case workspace under both execution identities and the
  worker sandbox. This command does not repair or validate a workspace.
- Read current ledger evidence for the exact case key, state revision, pending
  direct builder effect ID, and failed-attempt count. The case must have an
  accepted plan, no accepted PR/head, and no running/completed direct attempts.
  It must either be `READY_TO_BUILD` with one unleased pending builder effect,
  or have just moved from that state to `ESCALATED` specifically because of
  direct-worker provider failures. In the latter case use the superseded
  builder effect ID. Other escalations and human holds cannot be reopened.
- The accepted policy's failure budget must be exhausted, but its original
  elapsed-time deadline must not have expired. Do not backdate recovery or
  extend/change the accepted policy to bypass either check.

## Authorize one retry

For a direct required security/performance reviewer that failed immediately
after entering review, `authorize-review-retry` uses the same arguments and
offline/root checks below. It requires a provider-failure escalation from that
exact review revision, the retained accepted builder, and unchanged plan, PR
and head. It returns the case to `WAITING_CI`: fresh live authorization, CI,
required reviews and final preflight still apply. It cannot reopen another
kind of escalation or spend more than one extra failure allowance. This is
not permission to rewrite a result or repair the ledger with SQL.

Do not downgrade a ledger containing `REVIEW_RETRY_AUTHORIZED` to a release
that does not understand that event's allowance. Keep the matching release
and history together during recovery.

Run on the host as root, substituting independently verified values below.
Use a unique stable request ID for this authorization; reuse it if the command
response is lost. Paths and identifiers are examples, not pilot constants.

```sh
sudo /opt/pip/current/bin/pip-control authorize-builder-retry \
  --policy /etc/pip/repositories/REPOSITORY.json \
  --database /var/lib/pip/ledger.db \
  --direct-queue /var/lib/pip/direct-queue \
  --case 'VERIFIED_CASE_KEY' \
  --expected-revision VERIFIED_STATE_REVISION \
  --effect-id 'VERIFIED_FAILED_BUILDER_EFFECT_ID' \
  --expected-failures VERIFIED_FAILED_COUNT \
  --request-id 'operator-builder-retry-UNIQUE_ID' \
  --reason 'Describe the diagnosed failure, repair, and verified evidence'
```

The CLI uses the actual effective UID and host clock; neither can be supplied
as an argument. It rejects active/enabled execution units, nonempty direct
inbox/results, changed runtime/model bindings, and stale/conflicting requests.
The ledger transaction checks the accepted policy and live state again.

`Applied` means one immutable event, one state-revision increment, supersession
of the old pending task, and one new `DISPATCH_BUILDER` effect. `Replayed` means
that same authorization was already recorded; no further allowance is granted.
The old plan, task payload, failures, escalation, policy revision, and creation
time remain. The case returns to `READY_TO_BUILD`.
The effective failure limit for this case becomes its observed failures plus
one; other cases and repository policy are unchanged.

## Resume separately

Inspect the ledger and preserve a backup before activation. Restore the case's
already accepted active policy through the normal reviewed activation procedure;
do not mint a new policy revision for this operation. Live GitHub authorization,
workspace verification, exact model, and normal dispatch gates still apply.
The controller generates a new revision-bound task with current release skills
and the existing accepted plan. Another failure reaches the new case-specific
limit and normal controller reconciliation escalates it. The original case
deadline can also stop it, including while paused.

Recovery preserves existing builder commits and unfinished edits on the assigned
branch, provided its head still descends from the accepted base. It does not
reset source or accept the previous worker's claims. The next worker must finish
and validate the checkout; publication still requires an exact clean head.

This command never starts services or provider processes. A successful command
is not evidence of a successful build or PR.

The event uses the existing schema-8 event log, but older binaries do not
understand its retry allowance. Do not downgrade an authorized live ledger to
a pre-retry release; retain a compatible release for rollback, or remain paused
and explicitly assess recovery. A schema-version match alone is not proof of
behavioral compatibility.
