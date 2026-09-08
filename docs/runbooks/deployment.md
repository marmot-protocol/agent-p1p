# Deployment and recovery

This runbook describes the Rust installation boundary, not permission to act on
a host. Check [implementation status](../implementation-status.md) and live
state before changes. Historical evidence is under ../evidence/; old numbered
activation scripts are not the normal deployment interface.

## Normal release path

A push to master automatically runs .github/workflows/release.yml. Rust tests,
Clippy and the disposable systemd lifecycle gate must pass before the separate
signing job can access its protected environment secret. There is no routine
human approval gate for building releases. PR workflows cannot use this path.

The artifact is named pip-FULL_SOURCE_COMMIT. Its envelope contains:

- pip-release.tar.gz, with the binary, canonical skills, policy seed, units,
  signed manifest and signature;
- the exact installer, also covered by the signed manifest;
- SHA256SUMS for transfer checking.

The signed manifest binds the source commit, binary and resource digests.
Outer checksums alone are not authenticity proof. The permanent trust anchor
on Pirate is /etc/pip-release-public.key; its reviewable source is
config/release-public.key. Do not generate a fresh signing key for an ordinary
upgrade. Key provisioning or rotation is a separate operator action.

Building an artifact does not deploy it, select an issue, enable work or merge
a PR. Live authority remains separate.

## Verify before installation

Download the artifact for one successful run and verify its exact source.
Extract and transfer it with file modes preserved; for example, scp -p
preserves modes while a plain copy may not. Verify on the destination host
with an already-trusted executable and the permanent public key:

~~~sh
sudo /opt/pip/current/bin/pip-control verify-release \
  --release-root /staging/pip-release/root \
  --manifest /staging/pip-release/release-manifest.json \
  --signature /staging/pip-release/release-manifest.sig \
  --public-key /etc/pip-release-public.key
~~~

The JSON result supplies the manifest and binary SHA-256 values. Check the
source commit against the selected run. A mode, digest or signature failure is
a stop, not an invitation to bypass verification.

Copy only the verified cohort's installer to the root-owned executable path,
then verify that copy's digest against the signed resource before running it.
For the first host install, establish the trusted verifier/public-key bootstrap
explicitly; do not assume an unverified downloaded executable can verify itself.

## Upgrade an existing host

1. Record installed source, current release target, active policy and service
   states. Inspect ledger attempts and actual worker processes independently.
2. Stop new dispatch. Let active workers finish, or use an explicitly authorized
   termination/recovery procedure. Do not kill a running Hermes child merely
   to upgrade Pip.
3. Quiesce the controller and consumer before taking a consistent ledger
   snapshot. Isolated ingress may continue spooling verified deliveries.
4. Run the verified root-owned installer with the digests from verification:

~~~sh
sudo /usr/local/sbin/pip-install-release \
  --cohort /staging/pip-release \
  --public-key /etc/pip-release-public.key \
  --manifest-sha256 MANIFEST_SHA_FROM_VERIFICATION \
  --binary-sha256 BINARY_SHA_FROM_VERIFICATION
~~~

5. Verify installed provenance, ledger/history continuity, policy bytes and
   ownership, and service access. Reconcile managed Hermes profiles only when
   the release requires it and the runtime is quiescent.
6. Resume only the previously authorized work. Check service results and finite
   next timer firings, then follow the same case through ordinary reconciliation.

The installer serializes with a host lock, copies the cohort into root-only
staging, checks the pinned digests again, and installs a content-addressed
release under /opt/pip/releases/. It backs up the ledger through SQLite, switches
/opt/pip/current atomically, and restores installation snapshots on failure.

Since source 0dcf1f9, a valid existing operator policy is preserved byte-for-byte;
the signed paused seed is written only on first install. Invalid existing
configuration stops installation instead of being replaced by defaults.
Enabled/active unit state is preserved at installer entry. If the operator
stopped services for maintenance, resumption is a separate explicit step.

Current job definitions must not be rewritten to match a release. Model,
profile, policy-revision or skill-content changes require compatibility proof;
the remaining frozen-job gaps are tracked in implementation status. Keep
releases needed for rollback or retained assignments. Never reset the ledger,
delete an attempt, or relabel an issue merely to make an upgrade proceed.

Schema 12 retains findings per accepted event and reviewer, allowing a stable
finding ID to recur after remediation (even on the same head). Existing finding
payloads, digests and head bindings stay unchanged; their new `event_id` is NULL
because older schemas did not record that association. New observations bind to
their accepting event. Duplicate IDs within one review still fail atomically,
and exact-result replay does not insert another observation. A saved review
blocked by the former case-global key can be accepted by normal reconciliation
after upgrade; no result edit, attempt reset or new model call is needed.

## First-install prerequisites

- Compatible Linux/systemd, architecture and libc for the actual artifact.
  The published Ubuntu-built artifact and a binary built inside a Debian
  lifecycle fixture are different compatibility evidence.
- Dedicated no-login control and worker identities, validated or created by the
  installer. The worker cannot open the ledger or controller credentials.
- Upstream Hermes code installed separately from its service-owned state.
  Do not fork Hermes or point its installer at Pip's managed runtime directory.
- Exact configured provider models, authenticated outside the repository.
  Never silently substitute an available model for the requested one.
- A mounted workspace filesystem with the configured free-space reserve;
  verify its boot persistence before allowing work.
- Canonical repository caches with only policy-bound remotes. A no-checkout
  clone intentionally has absent working files; ordinary dirty-worktree output
  is not by itself corruption of that cache.

Pirate currently uses /var/lib/pip/worktrees bound to /mnt/raid0/pip/worktrees.
The latter is RAID0 capacity storage, not redundancy. The ledger is outside
that mount. Policies specify storage reserves and terminal-workspace retention;
never delete active work or unpreserved commits.

Managed service state lives at /var/lib/pip/hermes. Both HERMES_HOME and
HERMES_KANBAN_HOME point there. Conversational/personal Hermes is separate and
must not be stopped or reconfigured by a Pip deployment.

Pip probes supported Hermes CLI capabilities and verifies profile configuration
through config get --json. Its dedicated gateway uses --external-supervisor.
Historical version pins are evidence snapshots, not a requirement to maintain
a fork or prohibit future upstream upgrades.

## Credentials and publication

The controller/PR author and two semantic review lanes use separate GitHub
identities. On Pirate the author uses a machine-account token, while each lane
uses a private GitHub App. Root-owned credential files reach the controller
through systemd LoadCredential. App metadata binds app/installation/repository
IDs; reviewer tokens are minted only when that capability is needed.

The controller uses credential-store identifiers, not mandatory absolute-path
loads. An absent identifier allows startup/collection; actual GitHub access or
review publication still requires its credential. This is systemd's documented
[credential lookup behavior](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html#LoadCredential=).
The ingress and webhook consumer retain their mandatory credential requirements.

Before upgrading from the older absolute-path controller unit, provision these
root-owned aliases in the root:root 0700 `/etc/credstore` directory. Keep the
existing root:root 0600 regular source files; other services still use them.
Validate both paths and refuse to overwrite an unrecognized existing entry.

| Credential-store entry | Existing source file |
|---|---|
| `pip-github-token` | `/etc/pip/github.token` |
| `pip-reviewer-general-app` | `/etc/pip/github-reviewer-general.app.json` |
| `pip-reviewer-general-key` | `/etc/pip/github-reviewer-general.pem` |
| `pip-reviewer-secperf-app` | `/etc/pip/github-reviewer-secperf.app.json` |
| `pip-reviewer-secperf-key` | `/etc/pip/github-reviewer-secperf.pem` |

For example, after validation, the following creates the token alias without
copying the secret:

```sh
sudo ln -s /etc/pip/github.token /etc/credstore/pip-github-token
```

Repeat for the explicit mappings above. The installer does not create
or rotate credentials. Alias provisioning is a one-time credential-layout step,
not a new policy revision or a reason to rewrite accepted jobs. Missing private
keys remain publication errors; missing App identity metadata can block review
publication because the controller must still validate distinct review Apps.

Workers do not get GitHub credentials. Builders commit locally. The controller
signs the accepted tree, then uses the verified askpass executable, policy-bound
remote and exact head lease to publish. Git hooks, credential helpers, URL rewrites and other repository
configuration cannot alter credential-bearing publication.

Provision a dedicated SSH **signing-only** key and register its public key on the
policy's automation account. Do not reuse an SSH login key or the Pip release
signing key. The installer does not create, rotate or register account keys.
Keep the private key and public identity under a root-only directory, e.g.
`/etc/pip/commit-signing/`, and expose root-managed credential-store entries
`/etc/credstore/pip-commit-signing` (private key) and
`/etc/credstore/pip-commit-signing-identity` (identity JSON). Root-owned aliases to
the protected files are supported by systemd. Never link them into worker homes.

The identity JSON has exactly `schema_version` (1), `actor_id` (the policy's
automation actor), `name`, `email` (verified for that GitHub account), and
`public_key` (the registered Ed25519 public key). Use root:root mode 0600 regular
files; the controller also accepts systemd's root:root 0440 credential mounts.
Older systemd service-UID:root 0400 copies are accepted only in the supplied
credential directory on a verified read-only filesystem, not as ordinary
service-owned configuration files.
The two controller CLI inputs are `--commit-signing-identity` and
`--commit-signing-key`. The unit resolves these through identifier-only
`LoadCredential` entries so absent signing capability does not prevent service
startup. Publication fails closed and retries with its accepted build unchanged;
there is no unsigned fallback. GitHub App private keys remain isolated from workers
and are read only when a review publication needs them.

Keep provider credentials in their own runtime state, never in policies, task
bodies, Git remotes, source files or release manifests.

## Canary and rollback

For a new canary, verify the current issue, scope, authorizing label actor,
repository/model bindings, empty or expected queue, storage and service health.
Authorize only the chosen issue through the generic label workflow. Maintain
the current bounded concurrency and human-only merge policy. A board's existence
does not authorize work.

On an installation failure, inspect the retained release/ledger/unit snapshots
and failure report before retrying. Preserve new evidence; do not restore an
old database casually after workers or consumers have resumed. Stop an uncertain
worker only after establishing its identity, not from a PID alone.

Report installed source, ledger case/attempt state, real execution observations,
PR head, CI, required reviews and final disposition separately. A successful
installation or running process is not an end-to-end success.

The repeatable Linux gate is scripts/test-systemd-lifecycle.sh. It covers fresh
install, reinstall, upgrade, injected rollback, reboot recovery, actual
service-identity workspace handoff, controller signing credentials/sandbox,
JIT requirements and policy preservation.
Run it for executable lifecycle changes, and still verify the actual signed
artifact on its intended host.
