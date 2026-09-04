# Pirate webhook-consumer evidence, 2026-09-04

This note records an inert live-host verification. It is not canary activation
authority and contains no credential values.

## Installed release

- Source commit: `6adbbdea4d93fdb979f1c3cc601b86c9243063e7`
- Release/manifest SHA-256:
  `ce6ed31e12f0b72eca0a75bcd7cfafe49b36494a22fc1290475c80411e504415`
- Binary SHA-256:
  `d26f051235dd158c5d539c34c1a8f3b56ec16fee575bafade40040034d732cb9`
- Release public-key SHA-256:
  `7f389516c13716dd925e650bd712482f4d6d269f31c0a0955dd365c56b727261`
- Installed target:
  `/opt/pip/releases/ce6ed31e12f0b72eca0a75bcd7cfafe49b36494a22fc1290475c80411e504415`

The cohort was built on Pirate from a verified Git bundle at the exact source
commit. The cohort verified 24 artifacts. The one-time signing key was shredded
after verification and was absent before privileged installation. The pinned
installer hash remained
`5d45ffccabb54f5efb877de20ad1aae9b085767880bb3af4aa348682582a0676`.
The installer reported `intake_enabled:false`, `dispatch_enabled:false`, and
`timer_state:"preserved"`.

## Inert policy

The installed policy exactly matched the release copy and reported:

```json
{"automation_actor_id":292420120,"autonomous_merge":false,"dispatch_enabled":false,"intake_enabled":false,"merge_mode":"shadow","repository_paused":true,"required_ci_contexts":["Required CI"],"reviewer_general_actor_id":323997422,"reviewer_secperf_actor_id":323998100,"revision":2}
```

## Pending-delivery discovery and correction

The first post-install consumer probe against the preceding release found two
pending, validly spooled MDK `issues/closed` deliveries for issues 1651 and
1652. The old consumer rejected them at the activation guard. This exposed a
head-of-line problem: ordinary signed issue activity could remain pending
forever even though it was not an intake-label event.

Strict failing tests were added for both required behaviors:

- a relevant label delivery received under disabled, paused, non-dispatching
  policy is committed as ineligible and creates no case or effect; and
- an authenticated issue action unrelated to the configured label is committed
  and retired without a live issue read.

The corrected exact release passed formatting, workspace clippy with warnings
denied, the complete locked workspace tests, supply-chain pin validation, and
the disposable-systemd clean-install/reinstall/upgrade/rollback/restart gate.

## Installed consumer result

Before the manual cycles, the production ledger had zero webhook deliveries,
cases, events, runs, task projections, and outbox effects, while the spool had
two pending envelopes. Three manual starts of the static consumer service
produced, in order:

```json
{"delivery_id":"00802250-a830-11f1-9139-2b7d608a47f4","intake":{"candidate":null,"delivery":"APPLIED","delivery_id":"00802250-a830-11f1-9139-2b7d608a47f4","mutation_count":1,"observed_at":1788510169,"report_format":1,"repository":"marmot-protocol/mdk","repository_id":1055628515},"report_format":1,"result":"PROCESSED"}
{"delivery_id":"14623090-a82d-11f1-9bc3-c62583197470","intake":{"candidate":null,"delivery":"APPLIED","delivery_id":"14623090-a82d-11f1-9bc3-c62583197470","mutation_count":1,"observed_at":1788510169,"report_format":1,"repository":"marmot-protocol/mdk","repository_id":1055628515},"report_format":1,"result":"PROCESSED"}
{"delivery_id":null,"intake":null,"report_format":1,"result":"EMPTY"}
```

Afterward, the spool had zero pending and two processed envelopes. The ledger
had exactly two webhook deliveries and still had zero cases, events, runs, task
projections, and outbox effects.

The live unit state remained:

```text
pip-webhook-ingress.service enabled active
pip-webhook-consumer@mdk.timer disabled inactive
pip-controller@mdk.timer disabled inactive
pip-direct-worker@mdk.timer disabled inactive
pip-hermes-gateway.service disabled inactive
```

This proves authentic non-intake issue events traverse and retire through the
installed ingress/spool/ledger boundary without activation. It does not prove
the configured-label live GitHub reread path; that still requires one
deliberately controlled `pip-ok` delivery while all activation controls remain
off.
