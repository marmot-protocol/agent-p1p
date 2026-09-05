# Pirate service-visible Rust verification

Date: 2026-09-05. Host prerequisite installed; production Pip remains inert.
No provider calls, Hermes modifications, account changes, policy changes,
release deployment, canonical checkout changes, or workflow dispatch.

## Installation

The canonical MDK checkout at
`25c24321af81e1b819678c3d39d2c1c9b57f331f` requests Rust `1.97.1`, rustfmt,
and Clippy. A clean service PATH initially could not resolve Cargo.

Fresh root-owned installation: `/opt/pip/rust`, approximately 665 MiB.
Rustup's minimal profile plus rustfmt/Clippy was installed for
`1.97.1-x86_64-unknown-linux-gnu` without changing operator shell files.

- rustc: `1.97.1 (8bab26f4f 2026-07-14)`
- cargo: `1.97.1 (c980f4866 2026-06-30)`
- rustfmt: `1.9.0-stable (8bab26f4f6 2026-07-14)`
- clippy: `0.1.97 (8bab26f4f6 2026-07-14)`
- verified rustup-init and installed rustup SHA-256:
  `dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71`
- installed wrapper SHA-256:
  `7786aab0da0fdc40beafb75d9eb44e7a3b0817bb70b7126537e26b0221d98f3f`

All installed regular files/directories were checked for root ownership and
absence of group/world write bits. Workers cannot modify the shared toolchain.
The wrapper regression failed before implementation, then passed for all eight
command names, argument preservation, cache preservation, unset target/toolchain
overrides, pinned rustup home, disabled auto-install, and unknown-name rejection.
Focused Clippy passed with warnings denied.

## Sandboxed proof

Transient units reproduced the installed service restrictions, including
`ProtectHome=yes`, `ProtectSystem=strict`, the respective path allow/deny lists,
no capabilities, namespace restrictions, and `MemoryDenyWriteExecute=yes`.
They additionally had runtime/memory bounds. They ran commands directly, not
models or production gateway/worker cycles.

Hermes identity (`pip-control`): a fresh library in private `/tmp` compiled,
ran its one unit test, passed `cargo fmt --check`, and passed offline Clippy
with warnings denied. Operator home, ledger, and provider home remained hidden;
worktrees and toolchain remained non-writable.

Direct identity (`pip-worker`): an archive of the exact MDK head above was
extracted into a disposable RAID-backed directory, without registering a Git
worktree or changing the canonical repository. `cargo test --locked -p
fs-private -j 2` compiled the real crate and dependencies. Its first parallel
run had **44 passes and one failure**:
`private_exclusive_file_lease_is_nonblocking_and_drop_releases_it` returned
`WouldBlock` when reacquiring after drop at `crates/fs-private/src/lib.rs:1113`.
This is recorded, not fixed or attributed conclusively by this toolchain task.
The same sandbox rerun with `--offline -- --test-threads=1` passed **45/45**.
This is not evidence of a green full MDK CI suite or reliable parallel tests.

The direct probe also verified rustfmt/Clippy availability, hidden ledger,
Hermes auth and operator Rust, non-writable toolchain, and refusal of
`cargo +0.0.0 --version` with `toolchain ... is not installed` (no download).

```text
PIP_RUST_HERMES_SANDBOX_OK
PIP_RUST_DIRECT_SANDBOX_OK
```

The direct Cargo download cache was approximately 262 MiB on the root
filesystem. It is retained for subsequent builds; no shared target directory
or automatic cache quota was introduced. See the
[storage caveat and upgrade procedure](../runbooks/rust-toolchain.md).

Disposable install staging and the 92 MiB MDK source/build probe were removed
after testing. The private `/tmp` Hermes fixture disappears with its transient
unit. The production gateway, controller, direct worker, and consumer timers
were still inactive after verification. The installed Pip release and ledger
were not upgraded.
