# Managed Hermes storage and offline planner lifecycle

Source candidate, not a live release installation or canary activation.

## Implemented

- Policy-bound per-projection storage with reserve/device/ownership/path
  checks before queue publication, stable replay and retained results.
- Offline-only retirement from a frozen ledger projection: stopped execution
  runtime, terminal case, retention, ownership and nested-filesystem checks.
  Disposable files are separate from results. No automatic online GC or hard
  disk quota is claimed.
- A narrow writable scratch subtree in the gateway unit; worktree source
  remains read-only and direct-worker permissions remain unchanged.
- Stock `lsp.enabled: false` in managed profiles; no Hermes fork or weakened
  memory-execution protection.
- Canonical inline planner text (16 KiB bound) in accepted results, immutable
  worker history and controller-published plan comments. This fixes the
  cross-identity handoff without exposing private Hermes directories to direct
  workers. Missing/oversized inline plans fail before acceptance for new
  storage-bound tasks. Old frozen tasks retain their old rules.

See the [storage runbook](../runbooks/hermes-storage.md).

## Pirate proof

The reusable `scripts/test-offline-planner-sandbox.sh` ran against stock Hermes
`29112bef099274229cadff79cdff7bf7b99c4b77`. Latest proof root:

`/var/lib/pip/worktrees/hermes-scratch/offline-P4YEMYfb`

The transient unit inherited the gateway's hardening properties, kept source
read-only, allowed writes only in the isolated probe, hid real ledger/provider
and Hermes state, and additionally restricted sockets to AF_UNIX with all IP
traffic denied. No credentials were copied and no model was invoked.

Results: `PIP_OFFLINE_PLANNER_SANDBOX_OK`, exit 0, approximately 2.0 seconds,
172.6 MiB peak memory. Cargo's locked/offline fixture test passed. Stock Hermes
claimed/completed one deterministic task and returned its original CLI JSON.
The report's `synthetic_offline` flag and zero provider calls distinguish this
from Astra inference. A first fixture run exposed a test title-binding mismatch;
the fixture was corrected, not the adapter's binding checks.

The latest untouched report, including inline plan text, was consumed by the
opt-in Rust `offline_sandbox_completion_is_ingested_exactly_once` test. It was
accepted once and a repeat ingestion was idle. Local tests separately exercise
production storage allocation, unsafe paths/low reserve, stable replay, active
case/runtime/retention refusal, disposable-only retirement, queue blocking on
allocation failure, inline plan bounds, and publication of the complete plan.

The new scratch parent was provisioned as a private controller-owned directory
on the existing RAID-backed worktree mount. Isolated probes and logs remain;
the old 9.7 GiB planner cache and both live case histories were not removed.
Tailscale requested an SSH check during the work; authentication was renewed.

## Remaining gate

Local release gates: `cargo test --workspace --locked` passed 304 tests (five
explicit external probes ignored by default); the latest sandbox report passed
its opt-in ingestion test separately. Workspace/all-target/all-feature Clippy
with warnings denied, formatting, diff whitespace, supply-chain pin tests,
Node 24 action checks and release-workflow contract checks passed.

The signed candidate still needs protected CI approval, installation and inert
profile reconciliation. No production policy/profile was changed, no execution
timer was enabled, and no task was reset. Before a fresh canary, retire/revoke
the previous live authorization through the normal audited path and resolve
the provider-rejection/access question. These offline checks do not establish
Astra inference availability or permission to replay the rejected request.
