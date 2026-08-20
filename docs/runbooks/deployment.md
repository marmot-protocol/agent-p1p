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
  "workflow_version": 2,
  "contract_version": 1,
  "built_at": "RFC3339 timestamp",
  "builder_identity": "..."
}
```

Trusted CI derives `source_commit`; the installer does not accept a free-form
operator assertion that can disagree with the artifact.

The checked-in `.github/workflows/release.yml` is a manual, protected release
workflow. It requires the `pip-release` environment and externally provisioned
`PIP_RELEASE_SIGNING_KEY_BASE64` and `PIP_RELEASE_PUBLIC_KEY` secrets. It checks
out the triggering SHA without persisted GitHub credentials, reruns the full
Rust gates, builds and verifies the cohort, creates a deterministic tar
envelope, and uploads it under the exact source SHA. Configuring those secrets
or approving a run is a separate release-operator action.

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
  --output /staging/pip-v2-release \
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
  --release-root /staging/pip-v2-release/root \
  --manifest /staging/pip-v2-release/release-manifest.json \
  --signature /staging/pip-v2-release/release-manifest.sig \
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
- Dedicated no-login `pip-v2-control` and `pip-v2-worker` identities. Only the
  former can open the ledger; only the latter receives direct-provider state.
- Sufficient disk for a new release, database snapshot, and rollback release.

The active runtime uses a dedicated service-owned Hermes root at
`/var/lib/pip-v2/hermes`, with both `HERMES_HOME` and `HERMES_KANBAN_HOME`
pointing there. The compatible Hermes gateway/dispatcher and every managed
profile used by Pip must observe that same root. Personal operator state under
`~/.hermes` is not an acceptable production dependency.

Credentials and provider secrets are provisioned outside this repository and
outside the release manifest. An active repository requires three distinct
GitHub identities and credentials: the controller/PR author,
`reviewer-general`, and `reviewer-secperf`. The two review tokens are delivered
only to the controller service and are never placed in worker profiles, prompts,
task metadata, or environments. Git branch publication invokes the signed
`pip-control` binary as askpass and gives Git a credential-file path, never a
token value in argv or environment.

## Install ordering

1. Resolve the exact signed release cohort.
2. Verify manifest signature and immutable source identity.
3. Verify every artifact digest before privilege escalation and again from the
   root-owned staging copy.
4. Probe host, Hermes, provider, filesystem, credential, and identity
   prerequisites without mutation.
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
  --cohort /staging/pip-v2-release \
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
  automation_actor_id: null
  reviewer_general_actor_id: null
  reviewer_secperf_actor_id: null
merge:
  mode: shadow
  autonomous: false
  method: squash
max_remediation_rounds: 3
max_case_elapsed_seconds: 86400
max_provider_failures: 3
max_repeated_finding_fingerprint: 2
```

After reviewed release installation, but before enabling any timer:

1. Provision `/var/lib/pip-v2/repositories/mdk` as a real checkout owned by
   `pip-v2-control`, with exactly one `origin` URL matching
   `https://github.com/marmot-protocol/mdk.git`.

   For the public MDK canary, the initial checkout is:

   ```bash
   sudo -u pip-v2-control env \
     HOME=/var/lib/pip-v2 \
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
       /var/lib/pip-v2/repositories/mdk
   ```
2. Provision the service-owned Hermes auth file and direct-provider state
   outside the release. Do not copy tokens into policy or profiles.
3. Run the exact installed bootstrap as `pip-v2-control`:

   ```bash
   sudo -u pip-v2-control env \
     HOME=/var/lib/pip-v2/hermes/home \
     HERMES_HOME=/var/lib/pip-v2/hermes \
     HERMES_KANBAN_HOME=/var/lib/pip-v2/hermes \
     /opt/pip-v2/current/bin/pip-control bootstrap-runtime \
       --policy /etc/pip-v2/repositories/mdk.json \
       --hermes-root /var/lib/pip-v2/hermes \
       --skills-root /opt/pip-v2/current/share/pip-v2/skills \
       --auth-source /var/lib/pip-v2/hermes/auth.json \
       --hermes /usr/local/bin/hermes
   ```

4. Retain the JSON capability/bootstrap output and verify every effective
   profile binding plus board visibility from the service-owned root.
5. Configure all three numeric GitHub actor IDs and the actual required MDK CI
   contexts. Empty required contexts are not acceptable canary policy.
6. Provision `/etc/pip-v2/github-webhook.secret` as a root-owned `0600` file.
   Configure a trusted TLS ingress or webhook relay to preserve the raw request
   body and invoke the exact installed binary with the GitHub delivery headers:

   ```bash
   pip-control webhook-intake \
     --policy /etc/pip-v2/repositories/mdk.json \
     --database /var/lib/pip-v2/ledger.db \
     --github-token /run/credentials/INGRESS/github.token \
     --webhook-secret /run/credentials/INGRESS/github-webhook.secret \
     --payload /run/pip-v2-webhooks/DELIVERY.raw \
     --delivery-id X_GITHUB_DELIVERY \
     --event X_GITHUB_EVENT \
     --signature X_HUB_SIGNATURE_256
   ```

   The paths and header placeholders are ingress-specific; never substitute a
   decoded/re-encoded payload. The command verifies HMAC before mutation,
   records the delivery ID and payload digest, and re-reads the exact issue from
   GitHub. The periodic controller remains the missed-delivery reconciler.
7. Run non-dispatching GitHub, Hermes, and direct-provider health/recovery
   probes.

Only after those checks and separate activation authorization:

1. Verify the legacy pipeline will not intake the selected issue.
2. Select one ordinary, repository-local, non-sensitive issue suitable for the
   planner to validate; do not encode it in policy.
3. Confirm no other open MDK issue currently satisfies Pip v2 intake policy.
4. Enable repository intake and dispatch with both active limits set to one.
5. Start `pip-v2-hermes-gateway.service`, then enable the
   `pip-v2-controller@mdk.timer` and `pip-v2-direct-worker@mdk.timer` units.
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
