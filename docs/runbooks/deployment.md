# Target deployment and rollback runbook

**Status:** Executable Rust release and lifecycle runbook; production activation remains unauthorized

This runbook defines the evidence and ordering the Rust implementation must
satisfy. The release builder, pinned installer, and disposable-systemd harness
are executable. A passing harness does not authorize installation on a live
host or MDK activation.

The legacy `scripts/install-control-plane.sh` is not the target installer.

## Release artifacts

A release cohort contains:

- the `pip-control` Rust binary for the target triple;
- canonical skills and language-neutral contracts;
- systemd units or deterministic unit templates;
- configuration schemas and database migration metadata;
- an immutable release manifest; and
- a verifiable signature/attestation for that manifest.

The cohort contains the non-dispatching shadow unit plus inactive controller,
direct-worker, and Hermes-gateway templates. The installer installs these unit
files but never enables or starts them. Installed code is not activation
authority.

The manifest binds:

```json
{
  "release_format": 1,
  "version": "...",
  "source_commit": "40 lowercase hex",
  "cargo_lock_sha256": "64 lowercase hex",
  "target": "...",
  "rust_toolchain": "...",
  "binary_sha256": "64 lowercase hex",
  "resources_sha256": "64 lowercase hex",
  "workflow_version": 3,
  "contract_version": 2,
  "built_at": "RFC3339 timestamp",
  "builder_identity": "..."
}
```

Trusted CI derives `source_commit`; the installer does not accept a free-form
operator assertion that can disagree with the artifact.

The checked-in `.github/workflows/release.yml` is a manual, protected deployment
build. It has no operator inputs: a dispatch from `master` checks out the exact
triggering SHA without persisted GitHub credentials and derives the deployment
identifier as `git-<12-character commit>`. Dispatches from any other branch fail
closed. The workflow requires the `pip-release` environment and externally
provisioned `PIP_RELEASE_SIGNING_KEY_BASE64` secret. The corresponding public
trust anchor is the reviewable `config/release-public.key` file; it is not a
secret. The workflow runs the Rust and disposable systemd gates without access
to the protected environment. Only after both verification jobs pass can the
protected job materialize the base64-encoded
32-byte signing seed, build and verify the cohort, and upload its deterministic
tar envelope with the exact installer and outer checksums. The installer is
itself an artifact in the signed manifest; the top-level executable is copied
byte-for-byte from that verified resource. The signing-key secret is the
canonical base64 seed itself, not a second base64 encoding.
Configuring the secret or approving a run is a separate release-operator
action.

## Pre-release gates

All gates bind to the exact release commit:

1. formatting and lint;
2. unit and property tests for the pure state machine;
3. contract, store, migration, and adapter tests;
4. offline end-to-end restart/replay simulations;
5. release build from the locked dependency graph;
6. artifact/manifest digest verification;
7. disposable-systemd clean install;
8. idempotent reinstall;
9. upgrade from the last supported release;
10. injected-failure rollback at every mutation stage;
11. service restart and database recovery;
12. non-dispatching live reconciliation smoke; and
13. review of the exact manifest and cohort digests.

Passing local tests does not imply these lifecycle gates or live shadow evidence
passed.

The disposable lifecycle gate is:

```bash
scripts/test-systemd-lifecycle.sh
```

It builds three independently signed cohorts from one exact candidate commit,
then proves clean install, reinstall, upgrade, host-finalization rollback, and
restart recovery in a privileged systemd container. It asserts that intake,
dispatch, and the reconciliation timer remain disabled after a fresh install.

## Build and verify commands

Create an offline Ed25519 signing key outside the repository, mode `0600`, and
derive its public key with a trusted local build:

```bash
target/release/pip-control derive-public-key \
  --signing-key /secure/release-signing.key
```

Store the emitted public key in a separate root-owned file. Build only from a
clean reviewed checkout:

```bash
scripts/build-rust-release.sh \
  --output /staging/pip-release \
  --version 0.1.0 \
  --built-at 2026-08-20T12:00:00Z \
  --builder-identity reviewed-builder \
  --signing-key /secure/release-signing.key \
  --public-key /secure/release-public.key
```

Before privilege escalation, use a trusted `pip-control` binary to verify the
cohort and retain its JSON output:

```bash
pip-control verify-release \
  --release-root /staging/pip-release/root \
  --manifest /staging/pip-release/release-manifest.json \
  --signature /staging/pip-release/release-manifest.sig \
  --public-key /secure/release-public.key
```

The output includes the signed source commit plus exact manifest and binary
SHA-256 values. Those two digests are mandatory inputs to the root installer;
the installer copies the cohort into root-only staging and checks them again
before it executes the staged binary.

## Host prerequisites

- Supported Linux/systemd version and architecture.
- A compatible Hermes executable, verified through capability probes rather
  than an assumed version string alone. Pip supplies its own isolated gateway
  unit and does not reuse a personal gateway.
- Required provider CLIs and exact configured models available.
- Root-owned credential files with documented mode and size bounds.
- Dedicated no-login `pip-control` and `pip-worker` identities. Only the
  former can open the ledger; only the latter receives direct-provider state.
- Sufficient disk for a new release, database snapshot, and rollback release.
- A real mount at `/var/lib/pip/worktrees`, backed by dedicated workspace
  storage rather than the operating-system filesystem. The checked-in MDK
  policy requires at least 500 GiB free and retains terminal worktrees for
  86,400 seconds.

On Pirate, `/mnt/raid0` is the intended NVMe workspace filesystem. Prepare a
bind mount before installing Pip. Because the service identities do not exist
yet, the bootstrap directories begin as root-owned. The installer adopts only
the exact empty layout below after verifying the ownership, modes, mount point,
and distinct filesystem; it rejects any extra entry or existing worktree:

```bash
sudo install -d -m 0755 /mnt/raid0/pip
sudo install -d -m 0770 /mnt/raid0/pip/worktrees
sudo install -d -m 0755 /var/lib/pip
sudo install -d -m 0770 /var/lib/pip/worktrees
sudo mount --bind /mnt/raid0/pip/worktrees /var/lib/pip/worktrees
findmnt --target /var/lib/pip/worktrees
```

Do not make the bind mount persistent until the source/target paths and
ownership have passed the installation probe. The controller, direct worker,
and Hermes gateway units all require this exact mount point; a plain directory
on `/` is deliberately insufficient. Before activation, add a reviewed
`/etc/fstab` bind-mount entry and prove an unmount/mount cycle without enabling
any Pip unit. Pirate's `/mnt/raid0` is RAID0 and therefore capacity storage,
not redundant storage; the authoritative ledger remains outside it and GitHub
remains the remote source of branch/PR state.

The active runtime uses a dedicated service-owned Hermes root at
`/var/lib/pip/hermes`, with both `HERMES_HOME` and `HERMES_KANBAN_HOME`
pointing there. The compatible Hermes gateway/dispatcher and every managed
profile used by Pip must observe that same root. Personal operator state under
`~/.hermes` is not an acceptable production dependency.

### Pinned Hermes install on Pirate

The Hermes compatibility snapshot verified on 2026-09-03 is release
`v2026.8.31` (Hermes Agent `v0.21.0`), peeled commit
`29112bef099274229cadff79cdff7bf7b99c4b77`. The installer at that exact commit
has SHA-256
`85ef536d455e51ab67aa74d79272efd49fe717597dbaadfd3cca179a905f4706`.
Re-verify both values before a later installation rather than silently moving
the pin.

Install root-owned Hermes code separately from Pip's service-owned runtime
state. In particular, do not point the upstream installer at
`/var/lib/pip/hermes`: it creates initial configuration and ownership that Pip's
strict bootstrap must treat as unmanaged. Use a credential-free installation
home instead, skip interactive setup, bundled skills, browser components, and
the separately downloaded Computer Use driver:

```bash
test "$(sha256sum /home/jeff/hermes-install-v2026.8.31.sh | awk '{print $1}')" = \
  85ef536d455e51ab67aa74d79272efd49fe717597dbaadfd3cca179a905f4706

sudo env HERMES_HOME=/var/lib/hermes-bootstrap \
  /home/jeff/hermes-install-v2026.8.31.sh \
  --branch v2026.8.31 \
  --commit 29112bef099274229cadff79cdff7bf7b99c4b77 \
  --skip-setup \
  --skip-browser \
  --skip-computer-use \
  --no-skills \
  --non-interactive

test "$(sudo git -C /usr/local/lib/hermes-agent rev-parse HEAD)" = \
  29112bef099274229cadff79cdff7bf7b99c4b77
sudo env HERMES_HOME=/var/lib/hermes-bootstrap /usr/local/bin/hermes --version
sudo env HERMES_HOME=/var/lib/hermes-bootstrap \
  /usr/local/bin/hermes kanban create --help | \
  grep -E -- '--workspace|--idempotency-key|--initial-status'
sudo env HERMES_HOME=/var/lib/hermes-bootstrap \
  /usr/local/bin/hermes gateway run --help | grep -- '--external-supervisor'
```

The verified Kanban boundary identifies boards by immutable `slug`, accepts
typed `worktree:<path>` workspaces, exposes task identity and ownership in
`kanban list --json`, and exposes completed attempt outcome, profile, and
metadata in the `kanban show --json` `runs` array. Pip probes the required CLI
flags before creating its board or any managed profile. Its custom systemd unit
runs the gateway with `--external-supervisor`, so Hermes exits back to systemd
for restart rather than spawning its own replacement.

Credentials and provider secrets are provisioned outside this repository and
outside the release manifest. An active repository requires three distinct
GitHub identities: the controller/PR author, `reviewer-general`, and
`reviewer-secperf`. The controller currently uses the dedicated machine-account
token delivered as `github.token`. Each reviewer uses a separate private GitHub
App. The controller receives that App's metadata plus PEM key through systemd
credentials, creates an RS256 JWT with bounded clock skew/lifetime, and mints a
repository-scoped installation token for that controller cycle. Reviewer tokens
are never persisted as configuration or placed in worker profiles, prompts,
task metadata, or environments. Git branch publication invokes the signed
`pip-control` binary as askpass and gives Git the controller credential-file
path, never a token value in argv or environment.

Each reviewer metadata file is non-secret JSON and must bind the installation
to the policy's numeric repository ID:

```json
{
  "app_id": 123456,
  "installation_id": 987654,
  "repository_id": 1055628515
}
```

Provision these as
`/etc/pip/github-reviewer-general.app.json` and
`/etc/pip/github-reviewer-secperf.app.json`. Provision the corresponding
private keys as `github-reviewer-general.pem` and
`github-reviewer-secperf.pem`, root-owned and mode `0600`. The two App IDs and
installation IDs must be distinct. The controller rejects unknown metadata
fields, zero IDs, repository drift, duplicate reviewer Apps, unsafe PEM modes,
invalid RSA keys, failed token responses, and non-201 token endpoints before it
opens the ledger.

## Install ordering

1. Resolve the exact signed release cohort.
2. Verify manifest signature and immutable source identity.
3. Verify every artifact digest before privilege escalation and again from the
   root-owned staging copy.
4. Probe host, Hermes, provider, filesystem, credential, and identity
   prerequisites without mutation.
   The filesystem probe must confirm `/var/lib/pip/worktrees` is a mount point,
   resides on a different device from the ledger, and satisfies the policy's
   free-space reserve.
5. Quiesce dispatch and wait for or explicitly handle active leases.
6. Snapshot:
   - active release target;
   - ledger and migration version;
   - unit files and service/timer state;
   - installed policy and resource links;
   - Hermes profile markers/capabilities; and
   - pending outbox/active-case summary.
7. Install into a new root-owned content-addressed release directory.
8. Run offline binary/resource/config probes from that exact directory.
9. Back up the ledger and apply migrations transactionally.
10. Install policy, unit, wrapper, and skill-link changes.
11. Atomically switch the current release pointer.
12. Leave the installed gateway, controller, direct-worker, and shadow timers
    stopped unless their prior state was already enabled during an upgrade.
13. Validate identity, ownership, socket, database, and read/write boundaries.
14. Run non-dispatching GitHub/Hermes reconciliation and compare expected state.
15. Restore only the prior enabled/active policy state. Never infer activation
    from the presence of a board or label.

Failure after mutation begins invokes rollback.

The reviewed operator invocation is:

```bash
sudo scripts/install-rust-control-plane.sh \
  --cohort /staging/pip-release \
  --public-key /secure/release-public.key \
  --manifest-sha256 MANIFEST_SHA_FROM_VERIFY_OUTPUT \
  --binary-sha256 BINARY_SHA_FROM_VERIFY_OUTPUT
```

The installer serializes with a host lock, validates or creates the isolated
service identity, refuses unsafe directory state, installs a content-addressed
release, uses SQLite online backup for rollback, preserves the timer's prior
enabled/active state on upgrade, and leaves a fresh timer disabled. Its root
transaction does not enable intake or dispatch.

## Rollback ordering

1. Stop new dispatch and route consumers.
2. Stop the failed control service.
3. Restore unit, policy, wrapper, and resource-link snapshots.
4. Restore the previous release pointer atomically.
5. Restore the database only when the migration contract says downgrade is not
   forward-readable; never overwrite new evidence casually.
6. Reload systemd.
7. Restore the prior enabled/active state.
8. Reconcile without dispatch and verify ledger/board/GitHub agreement.
9. Retain the failed release, logs, manifest, and snapshot until investigation
   completes.

Rollback must not delete a worker, branch, PR, database, or release whose
ownership is uncertain.

## Generic MDK canary activation

This section is a future authorized procedure. It must not be followed until
the remaining cutover gaps in
[`../implementation-status.md`](../implementation-status.md) are closed and
the active unit has passed its own install/rollback lifecycle tests.

The binary contains no canary issue. The installed MDK policy starts paused:

```yaml
repository:
  id: 1055628515
  owner: marmot-protocol
  name: mdk
  default_branch: master
board: pip-mdk
intake:
  label: pip-ok
  enabled: false
  paused: true
  held_issue_numbers: []
  repository_active_limit: 1
  global_active_limit: 1
dispatch_enabled: false
github:
  automation_actor_id: 292420120
  reviewer_general_actor_id: 323997422
  reviewer_secperf_actor_id: 323998100
merge:
  mode: shadow
  autonomous: false
  method: squash
max_remediation_rounds: 3
max_case_elapsed_seconds: 86400
max_provider_failures: 3
max_repeated_finding_fingerprint: 2
required_ci_contexts:
  - Required CI
workspace_storage:
  require_distinct_filesystem: true
  minimum_free_bytes: 536870912000
  terminal_retention_seconds: 86400
```

After reviewed release installation, but before enabling any timer:

1. Provision `/var/lib/pip/repositories/mdk` as a real checkout owned by
   `pip-control`, with exactly one `origin` URL matching
   `https://github.com/marmot-protocol/mdk.git`.

   For the public MDK canary, the initial checkout is:

   ```bash
   sudo -u pip-control env \
     HOME=/var/lib/pip \
     GIT_CONFIG_GLOBAL=/dev/null \
     GIT_CONFIG_NOSYSTEM=1 \
     GIT_TERMINAL_PROMPT=0 \
     git -c core.hooksPath=/dev/null \
       -c credential.helper= \
       -c http.proxy= \
       -c http.extraHeader= \
       -c http.sslVerify=true \
       clone --no-checkout --origin origin \
       https://github.com/marmot-protocol/mdk.git \
       /var/lib/pip/repositories/mdk
   ```
2. Provision the service-owned Hermes auth file and direct-provider state
   outside the release. Do not copy tokens into policy or profiles.
3. Run the exact installed bootstrap as `pip-control`:

   ```bash
   sudo -u pip-control env \
     HOME=/var/lib/pip/hermes/home \
     HERMES_HOME=/var/lib/pip/hermes \
     HERMES_KANBAN_HOME=/var/lib/pip/hermes \
     /opt/pip/current/bin/pip-control bootstrap-runtime \
       --policy /etc/pip/repositories/mdk.json \
       --hermes-root /var/lib/pip/hermes \
       --skills-root /opt/pip/current/share/pip/skills \
       --auth-source /var/lib/pip/hermes/auth.json \
       --hermes /usr/local/bin/hermes
   ```

4. Retain the JSON capability/bootstrap output and verify every effective
   profile binding plus board visibility from the service-owned root.
5. Verify all three numeric GitHub actor IDs against live account/App evidence,
   and verify `Required CI` is still the active GitHub Actions-sourced status
   check in the MDK default-branch ruleset. Empty or drifted required contexts
   are not acceptable canary policy.
6. Provision `/etc/pip/github-webhook.secret` as a root-owned `0600` file.
   The installer creates a no-login `pip-ingress` identity and this spool
   boundary:

   ```text
   /var/spool/pip-webhooks                         root:root              0711
     receipts/                                    pip-ingress:pip-control 2750
     pending/                                     pip-ingress:pip-control 2770
     processed/                                   pip-control:pip-control 0711
   ```

   `pip-webhook-ingress.service` receives only the webhook secret through
   `LoadCredential`. It cannot read the GitHub token, ledger, repository,
   Hermes root, provider state, or worker queues. The receiver binds only to
   loopback and atomically acknowledges validated raw requests into the
   delivery-ID-addressed spool:

   ```bash
   pip-control webhook-serve \
     --listen 127.0.0.1:8787 \
     --spool /var/spool/pip-webhooks \
     --webhook-secret /run/credentials/INGRESS/github-webhook.secret
   ```

   It accepts only `POST /github` with one each of `X-GitHub-Delivery`,
   `X-GitHub-Event`, and `X-Hub-Signature-256`, JSON content, a valid HMAC, and
   at most 4 MiB of raw body. Authenticated `issues` events are durably spooled.
   An authenticated GitHub `ping` is answered with `204 No Content` without
   creating a receipt or workflow input; all other event types fail closed.
   Requests have a ten-second deadline and four-request concurrency bound. It
   has no GitHub token, ledger, repository, Hermes, or provider access.

   `pip-webhook-consumer@mdk.timer` runs a separate `pip-control` oneshot at a
   bounded rate. Each invocation reads at most one canonical pending envelope,
   verifies its base64 encoding and SHA-256 digest, revalidates the HMAC, and
   re-reads the exact issue from GitHub. It commits the immutable delivery and
   intake result before atomically renaming the `pending/` directory entry into
   `processed/`; the immutable `receipts/` hard link remains in place. GitHub
   outages and crashes leave the item pending; a retry converges through ledger
   delivery-ID replay protection.
   The consumer's systemd writable sandbox names the common spool root so the
   hard link remains on one mount. Directory ownership and modes above still
   prevent the controller identity from modifying `receipts/`.

   For a manual diagnostic, the equivalent direct boundary remains:

   ```bash
   pip-control webhook-intake \
     --policy /etc/pip/repositories/mdk.json \
     --database /var/lib/pip/ledger.db \
     --github-token /run/credentials/INGRESS/github.token \
     --webhook-secret /run/credentials/INGRESS/github-webhook.secret \
     --payload /run/pip-webhooks/DELIVERY.raw \
     --delivery-id X_GITHUB_DELIVERY \
     --event X_GITHUB_EVENT \
     --signature X_HUB_SIGNATURE_256
   ```

   The paths and header placeholders are ingress-specific; never substitute a
   decoded/re-encoded payload. Do not enable either webhook unit until the
   secret, trusted TLS forwarding path, service-identity probes, and active
   repository policy are ready. The periodic controller remains the
   missed-delivery reconciler.
7. Run non-dispatching GitHub, Hermes, and direct-provider health/recovery
   probes.

Only after those checks and separate activation authorization:

1. Verify the legacy pipeline will not intake the selected issue.
2. Select one ordinary, repository-local, non-sensitive issue suitable for the
   planner to validate; do not encode it in policy.
3. Confirm no other open MDK issue currently satisfies Pip intake policy.
4. Enable repository intake and dispatch with both active limits set to one.
5. Start `pip-webhook-ingress.service` and `pip-hermes-gateway.service`, then
   enable the `pip-webhook-consumer@mdk.timer`, `pip-controller@mdk.timer`, and
   `pip-direct-worker@mdk.timer` units.
6. Have a trusted actor apply `pip-ok` to that one issue.
7. Observe the generic intake path create exactly one case and planner task.
8. Keep merge mode `shadow` and autonomous merge false throughout the trial.

If another issue becomes eligible, concurrency prevents its activation but the
operator should remove the unintended authorization and record the discrepancy.

## Canary evidence report

Report separately:

- installed manifest/source/binary identity;
- service and resource health;
- current immutable case revision and state;
- active or completed task/run identities;
- exact PR head, CI, and review evidence;
- discrepancies, retries, escalations, and provider health;
- final shadow recommendation; and
- publication/merge state.

Do not describe a shadow recommendation as merge-ready publication, and do not
describe current green checks as proof that historical or other-head gates
passed.

## Deactivation

1. Disable intake and dispatch in policy.
2. Reconcile and verify no task can be newly activated.
3. Preserve active task evidence and choose explicit finish/terminate handling.
4. Snapshot ledger, policies, installed manifest, units, profiles, and board.
5. Stop/disable services only after the snapshot is verified.
6. Do not remove the board, case database, release directories, branches, or PRs
   without separate explicit approval.
