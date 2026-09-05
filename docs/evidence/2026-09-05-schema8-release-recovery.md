# Signed schema-8 release and isolated recovery

Date: 2026-09-05. Candidate verified and staged, **not installed on Pirate**.

Subsequent update: JG separately approved the
[live inert installation](2026-09-05-schema8-live-install.md), which passed.
The record below describes the preceding isolated drill.

## Exact candidate

Source: `ab16528588acc1b50e1a30fdb303c6a025bc4a4a`.
Protected [deployment build 33958310437](https://github.com/marmot-protocol/agent-p1p/actions/runs/33958310437)
passed Rust verification, disposable-systemd lifecycle verification, and the
operator-approved build/sign job. All four pending implementation commits were
pushed before dispatch; no signing key was retrieved to Pirate or this checkout.

The existing installed `933ec69` verifier on Pirate independently accepted the
candidate signature under `/etc/pip-release-public.key`, its exact source, and
all 26 artifact digests. The top-level installer matches the signed installer
resource byte-for-byte. The packaged worker-contract reference is present.

| Artifact | SHA-256 |
| --- | --- |
| Manifest / release ID | `65f5b791c8ec7ebff5237a00ab4bdbe3e6232844a8758536971537c0e474aa6e` |
| Binary | `4d0358e02a64a0584e348da8121ef95e3a3bdc449fbacc72e47e6eb871b83968` |
| Installer | `8e54ec3f98a39fec01e85805b32b0738ab1295986058a8c677a3f54bf56c6eeb` |
| Archive envelope | `55395013382c45a6e19eb415115e03b995a9e5305bf7fba0a64ca02c2cb8d912` |

Pirate staging root:

```text
/home/jeff/.cache/pip-deployments/ab16528588acc1b50e1a30fdb303c6a025bc4a4a/run-33958310437
```

The verified cohort is `candidate/`, not `cohort/`. This is a staging directory,
not an instruction to activate or install it.

## Isolation and fixture

A SQLite online backup was made from the live database using a **read-only**
source connection. Only the completed backup was supplied to the drill. It is
retained root-owned, mode `0600`, under the private staging directory; it is
not in GitHub artifacts or this repository. The source has the abandoned
`repo:1055628515#891@3` case, two events, two evidence rows, two outbox rows,
and 35 webhook deliveries. There are no accepted worker runs.

The container had its own systemd, root filesystem, temporary workspace mount,
and identities, with `--network none` and a single **read-only** staging mount.
It had no live Pip state, credential, Docker-socket, or host-systemd mount.
Privileged mode and a private cgroup namespace match the existing systemd test
harness; this is operational isolation, not a claim about containment of
hostile privileged-container code.

The first image used the CI harness's Debian 12 base. The already-published old
binary refused to start because it requires `GLIBC_2.39`. The actual-artifact
drill therefore used Debian 13, matching Pirate:

- base `debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132`;
- derived image `sha256:ff3cf525ba2a02b9104fb49e29283379384909093f44f4008a9fe51449b74161`;
- systemd `257.13`, glibc `2.41`.

CI builds its own binary in the older container; that passing test does not
certify published Ubuntu-built binaries for Debian 12. Do not infer that host
compatibility from the Rust target triple alone.

## Results using both actual signed releases

1. Install the prior `933ec69` cohort in the empty container, then place the
   completed schema-7 ledger backup there with the service identity's ownership.
2. Attempt the candidate upgrade with a one-shot `systemctl daemon-reload`
   failure. This occurs after the ledger migration and release-link switch.
   Verify restoration of the old release, schema 7, byte-identical policy, and
   identical status. Compare every row in **all 14 legacy tables**, not just
   their counts. Result: passed.
3. Install the candidate successfully. Verify schema 8, SQLite integrity and
   foreign keys, preservation of all legacy rows, and empty new
   `dispatch_batches`/`dispatch_create_attempts` tables. Result: passed.
4. Reinstall the exact candidate. Result: `existing`, with unchanged history.
5. Try the old reader and old installer against schema 8. Both specifically
   refuse `unsupported schema version: 8`; the candidate remains installed
   and all rows are preserved. Result: passed.
6. Restart the container and recheck the candidate, schema, all rows, and
   disabled/inactive execution units. Result: passed.
7. With all execution still stopped, checkpoint/close the disposable database,
   restore the pre-upgrade backup, and install the old cohort. Its status is
   identical to the original schema-7 status. Result: passed.

```text
SCHEMA7_FAILED_UPGRADE_ROLLBACK_OK
SCHEMA8_UPGRADE_REINSTALL_DOWNGRADE_REFUSAL_OK
SCHEMA8_RESTART_RECOVERY_OK
SCHEMA7_OFFLINE_BACKUP_RESTORE_OK
```

The diagnostic scripts needed two fixture corrections, not product changes:
open the completed read-only-mounted backup as immutable so SQLite does not
try to create WAL sidecars there, and checkpoint/close empty WAL/SHM sidecars
left by read-only validation before replacing the disposable database during
offline restore. Initial diagnostic logs are retained alongside the passing
`schema8-drill.log` and `schema8-restart-restore.log`.

The retained scripts have SHA-256 values:

- `drill.sh`: `d2bf5a0f6e6b0a0e6986cb413b041381b968b3bc0ce06b6534442446c0d2da31`
- `compare-ledgers.py`: `77d4cc44cb7d9258345916fb0982a2ea2687ab1341bdff9f89bf77455d872d09`
- `systemctl`: `fd35d4fc3fc417df3eb1ccfe8286f383e29f4c991b04af4ecb32618d31cc63ff`

The disposable container and unused Debian-12 drill image were removed after
verification. Artifacts, protected backup, scripts, and logs remain staged.

## Production unchanged / next boundary

Before and after the drill, Pirate still pointed to release
`6c2f1663af7cfabc024780f2ee7bb95379ed16a601b48584e90cfd27f68ff14d`, source
`933ec69cf530a6f5e119f8f62cad62832e53a3fd`. Its schema-7 status was identical.
The controller, direct-worker and webhook-consumer timers and dedicated worker
Hermes gateway remained inactive.

Unchanged live SHA-256 values:

- ledger: `da9c8e28c62bba6081b1a81e73556428eb85dc2e555ed73a65c5b8f9a2023862`;
- inert policy: `9750f3bd6d3d3cd0216ae0ab19858719de60f371d694899e49169a1231c34196`;
- release public key: `f7a0f0eca7c35e29b58d2c07212524f3bacd64f2c3a80ae3f2b8bab466ecd34b`.

Live installation remains a separate operator-approved action, followed by
read-only verification and runtime bootstrap while still inert. Activation
and the abandoned-case disposition remain separate decisions. Restoring an old
backup after new work has been accepted would discard that work: this drill
does **not** authorize that recovery procedure on a running system.
