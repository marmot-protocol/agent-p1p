# Pirate schema-8 live installation (inert)

Date: 2026-09-05, verified approximately 09:58 UTC. JG approved installing the
verified candidate **without enabling execution**, after the
[isolated signed-release recovery drill](2026-09-05-schema8-release-recovery.md).

## Installed identity

- Source: `ab16528588acc1b50e1a30fdb303c6a025bc4a4a`.
- Release/manifest SHA-256:
  `65f5b791c8ec7ebff5237a00ab4bdbe3e6232844a8758536971537c0e474aa6e`.
- Binary SHA-256:
  `4d0358e02a64a0584e348da8121ef95e3a3bdc449fbacc72e47e6eb871b83968`.
- `/opt/pip/current` resolves to that content-addressed release directory.

Before installation, the existing verifier rechecked the candidate signature
and all 26 artifact digests under the unchanged permanent public key. The
existing root-owned installer already matched the signed installer hash, so
it did not need replacement. The installation required no signing key, new
credential, Hermes modification, or additional user shell commands.

The installer reported `installed`, intake disabled, dispatch disabled, and
timer state preserved. Post-install verification against the **resolved real
release directory** accepted all 26 artifacts. The verifier intentionally
rejects `/opt/pip/current` as a release-root argument because it is a symlink.

## Ledger and runtime verification

A fresh pre-install SQLite backup was created through a read-only connection
and retained root-owned, mode `0600`, at:

```text
/home/jeff/.cache/pip-deployments/ab16528588acc1b50e1a30fdb303c6a025bc4a4a/run-33958310437/pre-live-ledger.db
```

The live ledger is now schema 8, still `pip-control:pip-control`, mode `0600`.
Every row in all 14 preexisting tables matches the backup, excluding only the
expected new schema-migration record. Integrity and foreign-key checks passed;
both new dispatch tables are empty. The abandoned case, two events, two
evidence rows, two outbox rows and 35 deliveries were preserved. No new worker
run, task projection, or PR was created.

Installed `bootstrap-runtime` reconciled all three managed Hermes profiles at
inert policy revision 3, creating no profile or board. The packaged worker
contract reference is readable through the planner's managed skill link. The
shared worker authentication file's digest was unchanged by bootstrap.

The controller, direct-worker and webhook-consumer timers and services,
dedicated worker Hermes gateway, and shadow timer remained inactive; the
execution timers and gateway remained disabled. The inert policy hash remains
`9750f3bd6d3d3cd0216ae0ab19858719de60f371d694899e49169a1231c34196`.

Webhook ingress remained active and its running executable resolves to the
new release. Conversational Pip's user-level Hermes gateway remained active.
Stock Hermes is still clean at commit
`29112bef099274229cadff79cdff7bf7b99c4b77`. The release trust anchor is unchanged.

```text
LIVE_SCHEMA8_ALL_14_LEGACY_TABLES_PRESERVED
LIVE_SCHEMA8_INSTALL_AND_INERT_BOOTSTRAP_OK
```

The private staging directory retains `live-install.sh`, `live-install.log`,
the signed cohort and fresh backup. The old release is retained for controlled
recovery, not for an unsupported direct schema downgrade.

## Remaining boundary

Installation is complete; workflow activation is not. The existing abandoned
canary and orphan gate card remain historical evidence, not work to delete or
silently revive. A fresh supervised canary requires a separately approved
issue/authorization and activation plan. MDK remains shadow/human-merge-only.
