# Pirate inert installation evidence — 2026-09-03

**Evidence boundary:** exact signed local-bootstrap Rust cohort installed on the
Pirate Linux/systemd host, with a persistent workspace bind mount and no active
Pip runtime. This is not protected-CI release evidence, a reboot test, a live
Hermes/provider probe, GitHub credential evidence, webhook ingestion, or a
canary run.

## Installed cohort

- Source commit: `9e2c9ee56e07f77dde8e2b25baaa6c74a7c9318f`
- Release/manifest ID:
  `e478e014bdfcbf4b7813a24dbb7d19d3a120b44c7f26d3e227e11174a55fa4ea`
- Installed binary SHA-256:
  `0346f31e591bde43e072507c2c4bb6af453051ebe0c4269c37dd899514479b6d`
- Installer SHA-256:
  `73828a923632a44da2a9d94abc84bc53d8958a7df623d460a77d9580b477c44c`
- Bootstrap public-key SHA-256:
  `651e0fc5016d073ab638667615a279ace1f6445b9f18b80751f22899fb62ac34`
- Installed release target:
  `/opt/pip/releases/e478e014bdfcbf4b7813a24dbb7d19d3a120b44c7f26d3e227e11174a55fa4ea`

The root-staged installer was mode `0555`; the installed bootstrap public key
was root-owned mode `0444`. The installer returned `installed` and preserved
the disabled intake, dispatch, and timer state. This cohort was signed by the
temporary operator-controlled local-bootstrap trust boundary, not the future
protected `pip-release` CI environment.

## Identity and ledger boundary

The post-install ownership probe reported:

| Path | Owner | Mode |
|---|---|---:|
| `/var/lib/pip` | `pip-control:pip-control` | `0700` |
| `/var/lib/pip/worktrees` | `pip-control:pip-control` | `0770` |
| `/var/lib/pip/hermes` | `pip-control:pip-control` | `0700` |
| `/var/lib/pip/repositories` | `pip-control:pip-control` | `0700` |
| `/var/lib/pip/artifacts` | `pip-control:pip-control` | `0770` |
| `/var/lib/pip/provider-home` | `pip-worker:pip-control` | `0700` |
| `/var/lib/pip/ledger.db` | `pip-control:pip-control` | `0600` |

The exact installed binary opened the ledger as `pip-control` and returned a
healthy schema version 6 with no cases, events, evidence, findings, runs,
attempts, task projections, webhook deliveries, workspace retirements, or
outbox entries.

## Persistent workspace mount

The one reviewed `/etc/fstab` entry is:

```text
/mnt/raid0/pip/worktrees /var/lib/pip/worktrees none bind,nofail,x-systemd.requires=/mnt/raid0 0 0
```

`findmnt --verify --verbose` completed with no errors or warnings. An explicit
unmount followed by `mount /var/lib/pip/worktrees` succeeded, after which
`findmnt` reported `/var/lib/pip/worktrees` backed by
`/dev/md0[/pip/worktrees]` as read-write ext4. The generated
`var-lib-pip-worktrees.mount` unit was loaded and active. Exactly one matching
fstab entry existed.

This proves persistence configuration and a mechanical remount without
activating Pip. It does not yet prove boot ordering or recovery after a real
host reboot. `/mnt/raid0` is RAID0 capacity storage, not redundant storage; the
authoritative ledger remains outside it.

## Inert-state proof

All installed execution units were disabled and inactive:

- `pip-shadow-reconcile.timer`
- `pip-controller@mdk.timer`
- `pip-direct-worker@mdk.timer`
- `pip-hermes-gateway.service`

The final process probe found no Pip controller, direct worker, or Hermes
gateway runtime. No issue, task, branch, comment, pull request, review, or merge
was created by this installation.

## Gates still open

- Prove the workspace mount after a real reboot.
- Provision the canonical MDK checkout.
- Bootstrap and probe the Pip-owned Hermes root and exact provider models under
  the service identities.
- Provision and validate the controller and two reviewer GitHub App identities
  and credentials.
- Configure exact required MDK CI contexts.
- Complete and deploy isolated webhook spool consumption behind trusted TLS.
- Replace local-bootstrap signing trust with protected CI signing evidence.
- Obtain explicit authorization before enabling any unit or labeling a canary
  issue.
