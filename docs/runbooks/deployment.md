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

The current cohort contains both the installed non-dispatching shadow unit and
a staged active-controller template. The target installer intentionally does
not install or enable the active template yet. Packaging a unit is not
activation authority.

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
- Existing compatible Hermes gateway/dispatcher, verified through a capability
  probe rather than an assumed version string alone.
- Required provider CLIs and exact configured models available.
- Root-owned credential files with documented mode and size bounds.
- Dedicated control-plane service identity with no login and exclusive ledger
  ownership.
- Operator identity authorized to manage the Hermes profiles/board.
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
task metadata, or environments.

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
12. Start the control service with intake/dispatch still paused.
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
repository: marmot-protocol/mdk
board: pip-mdk
intake_label: pip-ok
intake_enabled: false
dispatch_enabled: false
github:
  automation_actor_id: null
  reviewer_general_actor_id: null
  reviewer_secperf_actor_id: null
max_active_cases: 1
merge_mode: shadow
autonomous_merge: false
```

After reviewed release installation and non-dispatching reconciliation:

1. Verify the legacy pipeline will not intake the selected issue.
2. Select one ordinary, repository-local, non-sensitive issue suitable for the
   planner to validate; do not encode it in policy.
3. Confirm no other open MDK issue currently satisfies Pip v2 intake policy.
4. Enable repository intake and dispatch with `max_active_cases: 1`.
5. Set `github.automation_actor_id` to the verified numeric identity used by
   the controller credential; activation fails closed while it is absent.
6. Have a trusted actor apply `pip-ok` to that one issue.
7. Observe the generic intake path create exactly one case and planner task.
8. Keep merge mode shadow throughout the trial.

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
