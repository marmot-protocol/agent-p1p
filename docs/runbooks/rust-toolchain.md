# Service-owned Rust toolchain

Rust is an independently provisioned host prerequisite, not part of the Pip
release cohort. Do not expose an operator's home to supply a compiler, modify
Hermes, or enable execution as part of provisioning it.

## Layout and selection

- `/opt/pip/rust/bin`: root-owned rustup and its native proxies.
- `/opt/pip/rust/rustup`: root-owned versioned toolchains and default setting.
- `/usr/local/libexec/pip-rust-tool`: root-owned copy of
  `packaging/pip-rust-tool`, mode `0555`.
- `/usr/local/bin/{cargo,rustc,rustdoc,rustfmt,cargo-fmt,cargo-clippy,clippy-driver,rustup}`:
  root-owned symlinks to that wrapper. Inventory collisions before installing;
  never overwrite an unrelated command.

The wrapper fixes `RUSTUP_HOME` and disables automatic toolchain installation.
It does **not** set `RUSTUP_TOOLCHAIN`, override a repository pin, or choose a
shared target directory. An unavailable repository toolchain fails until an
operator explicitly provisions it. These are the documented rustup
[environment controls](https://rust-lang.github.io/rustup/environment-variables.html)
and [selection rules](https://rust-lang.github.io/rustup/overrides.html).

Cargo caches default to the invoking worker's `$HOME/.cargo`; direct workers
use `/var/lib/pip/provider-home/.cargo`, while Hermes profile terminals use
their own profile home. Do not share writable Cargo configuration between
these identities. Default builder `target/` output stays inside the managed
worktree on RAID storage and follows its retirement policy. Read-only Hermes
roles must use writable scratch space if they need compilation, not make
their checkout writable. No global `CARGO_TARGET_DIR` is configured.

The persistent Cargo download caches are **not** covered by worktree
retirement or its free-space reserve. They currently reside on the root
filesystem. Check their sizes and root capacity before expanding concurrency;
there is no new cache quota or janitor in this change. Prune only while the
corresponding workers are stopped, and preserve credentials/configuration.

## Provisioning and upgrades

1. Read `rust-toolchain.toml` at the actual target repository head. Record the
   head and exact channel, components, and host target.
2. Fetch `rustup-init` and its `.sha256` from
   `https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/` over
   verified HTTPS into a new private staging directory. Verify the checksum
   before execution. This is an upstream HTTPS/checksum trust boundary, not a
   Pip release signature.
3. For a fresh installation only, create `/opt/pip/rust` root-owned and install
   the verified initializer there. Run it as root with a clean environment,
   `CARGO_HOME=/opt/pip/rust`, `RUSTUP_HOME=/opt/pip/rust/rustup`,
   `--no-modify-path`, an exact `--default-toolchain`, `--profile minimal`, and
   the required `--component` values. Do not source the generated shell file
   or alter an operator's shell configuration.
4. Install the reviewed wrapper and absent command symlinks described above.
   Check all regular files/directories below the installation remain owned by
   root and are not group/world writable (symlink permission bits are not a
   meaningful write-access test).
5. For an additional version, use the installed wrapper explicitly:

   ```bash
   sudo env -i HOME=/root PATH=/usr/local/bin:/usr/bin:/bin \
     /usr/local/bin/rustup toolchain install 1.97.1 \
       --profile minimal --component clippy,rustfmt
   ```

   Replace the version only after inspecting the target repository. Adding a
   version does not require changing the default. Keep versions required by
   retained worktrees; remove old versions explicitly as an operator.
6. Prove compiler, linker, Cargo, formatting, and Clippy execution in bounded
   transient units mirroring **all** restrictions from the installed direct
   worker and Hermes gateway units. Production units stay inactive. Check
   hidden operator homes/ledger, immutable toolchain, writable scratch/cache,
   repository pins, and failure of an unavailable version. A shell under
   `sudo -u` alone is insufficient proof of systemd compatibility.

This supplies Rust, not every native library, auxiliary command, cross-target,
or repository CI requirement. Validate those with the intended build workload.

See [Pirate verification](../evidence/2026-09-05-rust-toolchain.md).
