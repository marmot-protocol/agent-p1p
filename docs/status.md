# Pip status

Updated 2026-10-02. The target is [Pip architecture](pip-architecture-plan.md).

## Where things stand

Pip ran one MDK issue (#993, [PR #1726](https://github.com/marmot-protocol/mdk/pull/1726))
through planning, build, remediation, exact-head CI, required reviews and
final review to human-held readiness in September 2026. Other canary issues
stopped on Pip's own limits rather than on their work, and each stall was
fixed with an incident-specific offline recovery command.

The October 2026 reliability pass changed how Pip handles failure:

- Failures are classified. Provider throttling, outages, expired logins and
  lease expiry back off without spending budgets; work failures spend a
  per-stage budget with a cooldown; anything else parks the case.
- Hermes jobs that crash, block or return an invalid result are retried as a
  fresh task for the same job, with the previous error, instead of escalating
  or stalling silently.
- The controller fills result bookkeeping; workers report only their work.
- Paused cases post a specific explanation, free their issue slot and are
  resumed, replanned or abandoned with an `@<pip-login>` comment.
- A no-progress watchdog replaces the 24-hour wall clock as the catch-all for
  stuck cases; the age limit is now 7 days.
- CI is judged by the latest attempt of each check; readiness is held only for
  real merge conflicts.
- GitHub reads are skipped for idle paused cases and back off near the rate
  limit; the controller runs every 30 seconds.
- Prompts were rewritten around the engineering task; the Python prototype was
  removed.

Policy revision 13 (`config/activation/repositories/mdk-rev13.json`) carries
the new limits.

## Not yet deployed

Pirate last ran `7700a21` (ledger schema 10). This branch adds schema 14. The
plan is to deploy on a fresh ledger: existing cases are abandoned, not
migrated. See [parked cases](runbooks/parked-cases.md) for operation and the
[deployment runbook](runbooks/deployment.md) for installation.

## Live proof still needed

- Two MDK issues, one touching MLS or cryptography, reaching readiness with no
  operator action.
- A provider outage (for example a logged-out Cursor CLI) producing a
  `PROVIDER_UNAVAILABLE` park comment, then `@<pip-login> resume` from GitHub.
- `@<pip-login> abandon` freeing a slot, and re-labelling restarting the case.
- A real Hermes `kanban_block` and Hermes crash, to confirm the event shapes
  the reader classifies (fixtures are currently synthetic).
- Cursor throttling and usage-limit messages: authentication and unknown-model
  output were captured from the real CLI; throttling signatures were not.
- A day of GitHub rate-limit headroom in controller output.

## Remaining work

- Remove the remaining offline `authorize-*` recovery commands and their
  validators once resume is proven live.
- Resume after a human pushes a helping commit to Pip's PR (currently a
  terminal takeover).
- Purge build output of long-parked cases; reconsider the 500 GB free-space
  floor against the actual volume.
- Simplify further: one execution path instead of Hermes plus Cursor, a
  smaller state set, and a store split by responsibility.

Historical observations remain in [evidence/](evidence/) and Git history.
