# Hermes build storage and offline retirement

The controller keeps source read-only and assigns each Hermes projection a
SHA-256-named directory beneath the policy's `hermes_scratch_root`. The shipped
Linux unit permits `/var/lib/pip/worktrees/hermes-scratch`; provisioning creates
that directory as `pip-control:pip-control` mode 0700 on the worktree mount.
Other layouts need matching reviewed unit permissions, not a silent fallback.

Task `storage` schema 1 names:

- `source`: the existing case checkout, never a new Hermes worktree;
- `cargo_target`, `cargo_home`, `temporary`: disposable build directories;
- `results`: retained plan/review artifacts.

Allocation checks ownership, canonical real directories, the workspace
filesystem, and the policy reserve before dispatch. Existing directories need
an exact ownership marker; replay never clears their contents. Direct-worker
permissions are unchanged. This is admission control, not a hard per-process
disk quota. Workers must use their assigned paths; profile-cache fallbacks are
forbidden. Managed Hermes profiles disable stock `lsp.enabled` to prevent
optional auto-installs in the restricted sandbox.

The canonical planner handoff is `evidence.plan_markdown`, nonempty and at most
16 KiB. It is retained in the accepted run and digest-bound history bundle,
then included in the controller's issue comment. Direct builders/reviewers do
not need filesystem access to private Hermes results. `plan_artifact` remains
a retained local copy, not the sole implementation plan. Historical tasks
without a `storage` binding retain their old acceptance rules.

## Retirement

Retirement is intentionally **offline maintenance**, not a timer that deletes
files while workers run. Pause intake/dispatch, stop and disable all execution
timers and the worker gateway, then use the exact frozen projection ID:

```sh
sudo -u pip-control /opt/pip/current/bin/pip-control scratch-retire \
  --policy /etc/pip/repositories/mdk.json \
  --database /var/lib/pip/ledger.db \
  --projection 'EXACT_FROZEN_PROJECTION_ID' \
  --now "$(date +%s)"
```

The command reads the projection from the ledger; it does not accept an
arbitrary deletion path. It requires inert policy, no active/enabled execution
units, gateway MainPID 0, a COMPLETED/ABANDONED/TAKEN_OVER case, and expired
terminal retention (24 hours in the target policy). It checks the ownership
marker and rejects nested filesystems before removing only `disposable/`.
Results and a retirement receipt remain. Repeating retirement is safe; retired
scratch cannot be reused. Operator-started workers outside the managed units
must also be stopped; maintenance is not safe during concurrent manual work.

No online automatic scratch GC or hard quota is claimed. Keep this maintenance
step between supervised canaries; a continuously active deployment needs a
separate lease-aware GC design. The old planner profile cache is not in this
namespace and is never swept by this command.

## Offline host probe

From a reviewed checkout on Pirate:

```sh
bash scripts/test-offline-planner-sandbox.sh
```

It creates a uniquely named isolated probe on the worktree filesystem, uses
the checked-in gateway hardening properties with narrower write permissions,
blocks internet sockets, copies no credentials, compiles a tiny offline Rust
fixture, and completes a deterministic stock-Hermes task. Its result explicitly
marks model fields as synthetic (`provider_calls: 0`). No production board,
ledger, model, issue, or PR is used. Source, logs and result files remain for
inspection. The Rust follow-up test consumes the untouched CLI report:

```sh
PIP_OFFLINE_PLANNER_REPORT=/absolute/path/to/report.json \
  cargo test -p pip-control --test result_cycle \
  offline_sandbox_completion -- --ignored
```

This proves filesystem/tool/queue/result plumbing, not model behavior or
provider authorization. The isolated fixture can be removed explicitly after
review; it is not an authoritative ledger case and cannot be swept as one.
