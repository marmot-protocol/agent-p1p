# Protected release installation and recovery evidence — 2026-09-04

**Evidence boundary:** exact-head protected GitHub Actions build, independent
consumer verification, live injected-failure rollback, content-addressed Pirate
upgrade, permanent public-trust rotation, idempotent reinstall, inert Hermes
reconciliation, and webhook-ingress restart. This evidence does not authorize
controller, worker, consumer, gateway, dispatch, or canary activation.

## Protected build and independent verification

The candidate was exact commit
`48ac1e2b190118ba99b11a46ee7b4ba7029a44cb` on `master`.

- Ordinary CI passed in GitHub Actions run
  [`33877448501`](https://github.com/marmot-protocol/agent-p1p/actions/runs/33877448501).
- The protected `Build Pip deployment` workflow passed in run
  [`33877448030`](https://github.com/marmot-protocol/agent-p1p/actions/runs/33877448030)
  after its `pip-release` environment approval.
- Artifact ID: `9938656233`.
- GitHub artifact-envelope digest:
  `sha256:143f38db6b7f93de3e15d4ff7424191799ad4fc3ad628987d49346f61f958a89`.
- Signed manifest SHA-256 and content-addressed release ID:
  `2591b7c24e4d05cc80b62933208fe59ca1d9ad31123df56515dd76d982d684d9`.
- Installed binary SHA-256:
  `8bc59f826f4094279cd362dfc87ee782a2153aeb2a98e56ac41287d211a3f2b5`.
- Installer SHA-256:
  `8e54ec3f98a39fec01e85805b32b0738ab1295986058a8c677a3f54bf56c6eeb`.
- Permanent public-key-file SHA-256:
  `f7a0f0eca7c35e29b58d2c07212524f3bacd64f2c3a80ae3f2b8bab466ecd34b`.

The downloaded envelope passed its outer `SHA256SUMS`. Its tar paths had one
`pip-release/` root and no absolute or parent-traversal paths. A trusted local
binary verified the Ed25519 manifest and all 25 signed artifacts against
`config/release-public.key`. The exported installer was byte-for-byte identical
to `share/pip/install/pip-install-release` inside the signature boundary. The
manifest bound version `git-48ac1e2b1901`, source and builder identity to the
exact commit, workflow version 3, contract version 2, and canonical UTC build
time `2026-09-04T13:19:41Z`.

The same archive, signature, digests, identities, and installer equality were
then verified on Pirate before privilege escalation using Pirate's previously
installed trusted `pip-control` binary.

## Injected live failure and rollback

The pre-upgrade release was source
`e4cbd333ede4197c31349b9e7259ee670311ed1e` at:

```text
/opt/pip/releases/99099430157256bfb922226afe698c2a650fdeb5e3bcff0699e29f40ac68a78b
```

Before mutation, the ledger was schema 7 with eight webhook deliveries and no
cases, events, evidence, findings, runs, direct attempts, task projections,
workspace retirements, or outbox entries. Its physical file SHA-256 was
`a87d27e53a40490e7e8b574c8cc9fe0f9dbf24dcb35ed18784efa4c859b4b418`.

A root-owned one-shot `systemctl` wrapper forced the first post-mutation
`daemon-reload` to fail. The installer reported the expected host-lifecycle
failure and its rollback:

- restored the old release pointer and source;
- removed the incomplete new release directory;
- restored the installed policy;
- left the old canonical public trust anchor unchanged;
- restored the public webhook ingress to enabled and active;
- kept every controller, worker, consumer, gateway, and reconciliation unit
  disabled and inactive; and
- left no failed systemd units or temporary fault-injection files.

The rollback restored the ledger through SQLite's online-backup boundary. Its
physical file SHA-256 therefore changed to
`2d59a3e5e4756414c1bc319d106c12a41552831a04894113cda6d8c3030b0635`,
while a fixed-time `pip-control status` comparison proved the authoritative
schema and every semantic count were identical. Physical SQLite page layout is
not used as workflow-state evidence.

## Successful install and recovery checks

The same preverified cohort was then installed successfully. The installer
reported source `48ac1e2b190118ba99b11a46ee7b4ba7029a44cb` and release ID
`2591b7c24e4d05cc80b62933208fe59ca1d9ad31123df56515dd76d982d684d9`.
It preserved disabled intake and dispatch and the prior service states.

The operator then atomically replaced the old local-bootstrap public key with
the committed protected-CI trust anchor. An immediate reinstall under that
canonical key returned `result: existing`, proving the installed release was
idempotently recognized through the permanent trust boundary.

The exact installed binary reconciled the service-owned Hermes root against
policy revision 3 and pinned Hermes Agent v0.21.0. The existing board was
reused, no profiles were created, and all three managed native profiles were
reconciled. This did not dispatch a task.

Finally, the webhook ingress restarted successfully under the new release. An
independent read-only probe found:

- `/opt/pip/current` resolved to the new content-addressed release;
- `SOURCE.COMMIT` was the exact protected-workflow commit;
- the installed binary, policy, installer, and canonical public key had the
  expected hashes, root ownership, and read/execute modes;
- the ingress was enabled and active with `Result=success`, `NRestarts=0`, and
  its command bound to `/opt/pip/current/bin/pip-control` on loopback port
  8787;
- all controller, direct-worker, webhook-consumer, shadow-reconciler, and
  Hermes-gateway units remained disabled and inactive;
- systemd reported no failed units; and
- the fixed-time ledger status remained semantically identical, including all
  eight retained webhook deliveries.

The retained continuation log on Pirate had SHA-256
`b28612ec4468629908cb1a4ecec9ea3b34d9e41cfd8b0801018615f78b0b08a3`
and ended with `LIVE_PIP_INSTALL_RECOVERY_DRILL_OK`.

## Remaining authority boundary

Protected release provenance, permanent trust, installation rollback, upgrade,
idempotency, runtime reconciliation, and ingress restart recovery are now
closed gates. Enabling any inert execution unit and running the first MDK
shadow case still requires separate explicit authorization.
