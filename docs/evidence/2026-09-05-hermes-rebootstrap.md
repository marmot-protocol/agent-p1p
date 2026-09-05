# Stock Hermes re-bootstrap and worker documentation

Status: fixed in candidate source and verified against unmodified Hermes on
Pirate. Production remains inert; no deployment, provider call, or account
change was performed by this check.

## Ownership correction

The real planner probe had caused Hermes to seed `.bundled_manifest` and
bundled skill directories into the profile. Pip wrongly treated every entry
in `profiles/<role>/skills` as Pip-owned.

Bootstrap now manages only the policy's named skill links. It neither adopts,
follows, rewrites, nor deletes unrelated siblings. The profile ownership
marker, real-directory checks, and authentication-link checks remain. A file
or directory occupying a Pip skill-link path is rejected during preflight,
before CLI calls or configuration writes. Owned symlinks can still be
atomically repointed during a release upgrade.

The regression first failed against the previous code with `UnmanagedPath`.
It now proves preservation of the Hermes manifest, bundled manual, and an
unrelated skill, plus fail-closed behavior for an owned-path collision.

## Worker field guide

The canonical guide now lives at:

`skills/shared/workflow-contract/references/worker-result-contracts.md`

Every role points to this resource relative to the loaded shared skill, not a
file assumed to exist in MDK. The source documentation is a pointer; release
packaging also keeps a compatibility copy under `share/pip/docs`.

Hermes workers can reach the guide through their canonical skill symlink.
Direct Cursor workers receive its full contents alongside the workflow skill
in the immutable prompt. A missing guide fails before any Cursor provider
probe. Regression tests proved the old prompt omitted the guide before the
fix, then passed with the guide included and with missing-guide rejection.

## Real-host proof

Stock Hermes commit: `29112bef099274229cadff79cdff7bf7b99c4b77`.

The opt-in `pip-hermes` bootstrap test ran as `pip-control`, with a fresh
temporary home, a dedicated board, and an empty authentication fixture:

1. Bootstrap all three Hermes profiles with candidate skills.
2. Run real `hermes -p planner skills opt-in --sync`.
3. Re-bootstrap the same profiles and verify the manifest bytes are unchanged.
4. Repoint the managed skills to a second release root and verify the field
   guide remains readable and the Hermes manifest remains unchanged.

```text
STOCK_HERMES_REBOOTSTRAP_AND_RELEASE_RELINK_OK
test result: ok. 1 passed; 0 failed
```

This required no account credentials, model calls, upstream patch, or
production runtime activation. The full default workspace tests, Clippy with
warnings denied, formatting, and release-script shell syntax also passed.
Disposable remote build files were removed after validation.

## Remaining pre-activation requirement

Update: the toolchain requirement below was subsequently addressed by the
[dedicated Rust installation and sandbox checks](2026-09-05-rust-toolchain.md).
The following describes the pre-provisioning observation.

Pirate's Rust tools currently resolve through `/home/jeff/.cargo/bin/rustup`.
With the service's clean PATH, `pip-control` finds neither `rustc` nor `cargo`.
The service sandbox deliberately hides operator homes. Provision a dedicated,
versioned service-visible toolchain and writable per-worker caches, then prove
compilation under the actual service restrictions. Do not fix this by exposing
Jeff's home or disabling `ProtectHome`.

Release deployment and the abandoned-case recovery decision remain separate
from these isolated compatibility checks.
