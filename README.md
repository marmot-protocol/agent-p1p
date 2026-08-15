# agent-p1p

Canonical source for Pip v2: repository-scoped boards, deterministic orchestration contracts, direct Cursor worker adapters, versioned role skills, and the MDK pilot configuration.

## Status

Runtime foundation and an inert MDK shadow pilot. The empty `pip-mdk` board is archived so Hermes cannot dispatch it. New intake and dispatch remain disabled. The legacy `pip` board still owns existing work, and MDK autonomous merge remains forbidden.

## Runtime roles

| Role | Runtime | Model |
|---|---|---|
| `planner` | fresh Hermes profile | `openai-codex/gpt-5.6-sol`, `xhigh` |
| `builder-grok` | direct Cursor adapter | `cursor-grok-4.6-high` |
| `reviewer-general` | fresh Hermes profile | `openai-codex/gpt-5.6-sol`, `high` |
| `reviewer-secperf` | direct Cursor adapter | `kimi-k3-high` |
| `final-reviewer` | fresh Hermes profile | `openai-codex/gpt-5.6-sol`, `xhigh` |

The Cursor roles are not wrapped by another reasoning model. Every role starts a fresh session.

## Skill composition

`related_skills` metadata is not treated as dependency injection. Runtime composition is explicit and ordered:

1. `skills/shared/workflow-contract/SKILL.md`
2. the role-specific `skills/<role>/SKILL.md`
3. immutable task input and repository policy
4. the required result schema

Hermes profile skill directories symlink both canonical skills. Cursor adapters read and render both canonical files directly. Each adapter saves the rendered prompt and invocation metadata with the run artifacts.

## Layout

```text
config/boards/          Inert board policy
config/repositories/    Repository and pilot policy
manifests/roles/        Declarative runtime/model/tool/skill definitions
docs/                   Architecture and implementation plan
schemas/                Versioned worker/case JSON contracts
skills/                 Canonical shared and role-specific skills
src/pip_agent/          Contracts, bootstrap, adapters, intake, case store, state machine
tests/                  Contract, runtime, package, and offline E2E tests
```

## Commands

```bash
# Show the three Hermes profiles that bootstrap would manage
uv run pip-v2-bootstrap --repo-root "$PWD"

# Apply profile configuration and canonical skill links
uv run pip-v2-bootstrap --repo-root "$PWD" --apply

# Run a direct Cursor role from immutable task input
uv run pip-v2-cursor builder-grok \
  --repo-root "$PWD" \
  --task /path/to/task.json \
  --worktree /path/to/assigned/worktree \
  --artifacts /path/to/new/run-artifacts

# Exercise planner → build → parallel reviews → join → final shadow decision
uv run pip-v2-fixture

# Inspect one pip-ok issue without creating a case or task
uv run pip-v2-intake \
  --config config/repositories/mdk.json \
  --issue NUMBER

# Development checks
uv run --locked --dev pytest -q
```

The Cursor adapter refuses to reuse an artifact directory, always starts a new CLI session, requests Cursor sandboxing, does not auto-approve MCPs, verifies the pinned model is advertised exactly once, captures stdout/stderr, validates immutable task bindings and the returned role schema, and fails closed when the returned model identity differs. The installed Cursor CLI does **not** expose the actually routed model in its result envelope, so the adapter records that limitation rather than claiming cryptographic proof against an upstream silent substitution.

## Storage trust boundary

“Immutable runs” means append-only through the control-plane API, with SQLite
triggers and connection-scoped guards catching accidental direct-SQL mutation.
It is not cryptographic tamper evidence against the owner of the SQLite file: a
process with arbitrary write access can replace the database, drop triggers, or
register replacement SQLite functions. `CaseStore` creates the database as
mode `0600`, rejects symlinked or foreign-owned files, and the production state
directory must be exclusively writable by the deterministic control-plane OS
identity. Worker roles must never receive filesystem access to that path.

`pip-v2-control` provides the first activation slice. A dedicated systemd user
owns `/var/lib/pip-v2` and exposes a bounded Unix socket. The public protocol has
only two operations: idempotently create the single root-configured canary case,
and read its status. Callers cannot supply a repository, issue number, state,
run, transition, PR, or merge decision. This deliberately makes same-UID worker
access to the socket non-authoritative: it cannot select new work or mutate the
workflow. Lifecycle/result submission remains disabled until a later control
plane can validate evidence independently.

Copy and verify the installer as root before executing it; do not run the
user-writable checkout script directly:

```bash
sudo bash -c '
  set -euo pipefail
  src=$1; installer_sha=$2; shift 2
  pinned=$(mktemp /root/pip-v2-installer.XXXXXX)
  trap "rm -f -- $pinned" EXIT
  install -o root -g root -m 0700 "$src" "$pinned"
  printf "%s  %s\\n" "$installer_sha" "$pinned" | sha256sum -c -
  "$pinned" --installer-sha256 "$installer_sha" "$@"
' bash \
  /home/jeff/code/agent-p1p/scripts/install-control-plane.sh \
  <verified-installer-sha256> \
  --wheel /home/jeff/code/agent-p1p/dist/agent_p1p-0.1.0-py3-none-any.whl \
  --sha256 <verified-wheel-sha256> \
  --caller jeff \
  --issue 1240
```

The installer creates the system identity, installs a root-owned wheel release,
writes an exact shadow/human-merge-only policy, installs a hardened systemd
unit, starts it, and verifies the caller can reach the status endpoint. It does
not activate the `pip-mdk` board or enqueue a task.

## MDK pilot

The first target is `marmot-protocol/mdk` on board `pip-mdk` using intake label `pip-ok`. The pilot remains inert until an explicit activation step:

- `intake_enabled: false`
- `dispatch_enabled: false`
- `archived_until_activation: true`
- `merge_mode: shadow`
- `autonomous_merge: false`
- existing MDK work remains on the legacy board

No issue is eligible merely because the label exists; intake must also be explicitly enabled later. A shadow-ready result is a recommendation to JG, not merge authority.

Intake additionally verifies that `pip-ok` was applied by a configured trusted actor, rejects pull requests and explicitly held issue numbers, and uses a deterministic Kanban idempotency key. `--enqueue` is refused while `new_intake_enabled` is false.

See [`docs/pip-v2-architecture-plan.md`](docs/pip-v2-architecture-plan.md).
