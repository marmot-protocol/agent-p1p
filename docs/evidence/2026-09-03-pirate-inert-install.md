# Pirate inert installation evidence — 2026-09-03

**Evidence boundary:** corrected exact signed local-bootstrap Rust cohort
installed on the Pirate Linux/systemd host, with a persistent, reboot-verified
workspace bind mount, a canonical MDK checkout, and a successfully bootstrapped
service-owned Hermes root. No Pip runtime is active. This is not protected-CI
release evidence, a live provider API/outage probe, GitHub App credential
evidence, webhook ingestion, or a canary run.

## Installed cohort

- Source commit: `ff15894be798d403ea28ac668f82a15dd30575fd`
- Release/manifest ID:
  `89786823409d5d18d612d2fb10412e04c240d1d34a958ada94b6a8b4ac1a91cc`
- Installed binary SHA-256:
  `5dbc54ee21cbe937686f21bbeeb08b1ceef7f53628c1cac2ee866b66843133fc`
- Installer SHA-256:
  `73828a923632a44da2a9d94abc84bc53d8958a7df623d460a77d9580b477c44c`
- Bootstrap public-key SHA-256:
  `651e0fc5016d073ab638667615a279ace1f6445b9f18b80751f22899fb62ac34`
- Installed release target:
  `/opt/pip/releases/89786823409d5d18d612d2fb10412e04c240d1d34a958ada94b6a8b4ac1a91cc`

The root-staged installer was mode `0555`; the installed bootstrap public key
was root-owned mode `0444`. The installer returned `installed` and preserved
the disabled intake, dispatch, and timer state. This cohort was signed by the
temporary operator-controlled local-bootstrap trust boundary, not the future
protected `pip-release` CI environment.

This cohort superseded the initially installed `9e2c9ee` cohort after a
disposable live probe found that Hermes v0.21 renders `config get model` as a
default/provider pair. Commit `ff15894` corrected the compatibility check; the
corrected cohort passed the disposable bootstrap lifecycle before installation
on the real service root.

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

This initially proved persistence configuration and a mechanical remount
without activating Pip. A subsequent real reboot completed at
`2026-09-03 12:53:46` with boot ID
`0357cd54-7db6-436a-a803-ef5adb725d8b`. After that boot:

- `/var/lib/pip/worktrees` was mounted read-write from
  `/dev/md0[/pip/worktrees]` as ext4;
- `var-lib-pip-worktrees.mount` was loaded, active, and mounted from the
  generated systemd unit;
- the generated unit required and ordered after `mnt-raid0.mount`;
- systemd reported zero failed units; and
- the fstab entry count remained exactly one.

This closes the mount boot-order and reboot-recovery gate. `/mnt/raid0` is
RAID0 capacity storage, not redundant storage; the authoritative ledger remains
outside it.

## Canonical checkout and isolated Hermes bootstrap

Before bootstrap, the service-owned inputs were validated as follows:

- `/var/lib/pip/repositories/mdk` was a `pip-control:pip-control` mode-`0775`
  Git worktree;
- its only remote was `origin`, exactly
  `https://github.com/marmot-protocol/mdk.git`;
- Git reported `true` for `--is-inside-work-tree`; and
- `/var/lib/pip/hermes/auth.json` was a non-empty
  `pip-control:pip-control` mode-`0600` regular file.

The exact installed `pip-control` binary then ran `bootstrap-runtime` as
`pip-control`, with both `HERMES_HOME` and `HERMES_KANBAN_HOME` bound to
`/var/lib/pip/hermes`. It reported:

```json
{"ok":true,"policy_revision":1,"repository":"marmot-protocol/mdk","runtime":{"board_created":true,"hermes_version":"Hermes Agent v0.21.0 (2026.8.31)","profiles_created":3,"profiles_reconciled":0}}
```

The bootstrap created and re-probed the `pip-mdk` board, created the managed
`planner`, `reviewer-general`, and `final-reviewer` profiles, and verified each
effective model/provider, reasoning-effort, and terminal-home binding. It did
not start a gateway, dispatch a task, exercise the provider API, or enable a
systemd unit.

## Inert-state proof

All installed execution units were disabled and inactive:

- `pip-shadow-reconcile.timer`
- `pip-controller@mdk.timer`
- `pip-direct-worker@mdk.timer`
- `pip-hermes-gateway.service`

The final process probe found no Pip controller, direct worker, or Hermes
gateway runtime. No issue, task, branch, comment, pull request, review, or merge
was created by this installation.

The same inert conditions held after the real reboot and after the corrected
cohort upgrade: every execution unit was still disabled and inactive and the
runtime process count was zero. The operator account could not traverse the
mode-`0700` service data root, and passwordless sudo was not available;
therefore the unprivileged remote probes did not reopen the ledger as
`pip-control`. The successful pre-reboot service-identity ledger probe above
remains the ledger evidence.

## Gates still open

- Provision and validate the controller and two reviewer GitHub App identities
  and credentials.
- Configure exact required MDK CI contexts.
- Complete and deploy isolated webhook spool consumption behind trusted TLS.
- Replace local-bootstrap signing trust with protected CI signing evidence.
- Record a non-dispatching live provider capability and outage/recovery probe,
  and confirm the separately supervised gateway observes the same Hermes root.
- Obtain explicit authorization before enabling any unit or labeling a canary
  issue.
