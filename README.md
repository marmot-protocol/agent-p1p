# agent-p1p

`agent-p1p` is the source repository for Pip v2: a repository-scoped,
deterministic control plane that turns authorized GitHub issues into planned,
built, independently reviewed, and finally adjudicated pull requests through
Hermes Kanban boards.

## Status

The repository is migrating from a Python single-issue prototype to the target
Rust control plane. Phases 0 through 5 and the Phase 7 non-dispatching release
lifecycle are implemented locally. Phase 6 is partial: the GitHub write
adapters are tested, authorization removal and takeover are durable, and the
final-review evidence gate is wired; several write effects are not yet consumed
by the active controller. Phase 8 has live read-only GitHub evidence,
but not live Hermes/provider evidence. No live MDK intake or dispatch has been
authorized.

The Python implementation is useful as a safety prototype and behavioral
reference, but it is not the target runtime and must not be installed from the
current branch. At the migration baseline (`2607004`), upstream CI was red and
the route-DAG upgrade was incomplete. The existing MDK automation remains a
shadow, human-merge-only canary.

The documentation distinguishes three things explicitly:

- **Target architecture:** the generic Rust system we intend to build.
- **Legacy prototype:** the current Python implementation, including its
  hard-coded `marmot-protocol/mdk#1240` behavior.
- **Activation policy:** the operational controls that limit a generic engine
  to one deliberately tagged canary issue without encoding that issue in code.

## Documentation

- [`docs/pip-v2-architecture-plan.md`](docs/pip-v2-architecture-plan.md) —
  canonical target architecture and invariants.
- [`docs/current-python-canary.md`](docs/current-python-canary.md) — accurate
  inventory of the legacy Python prototype and its gaps.
- [`docs/control-flow.md`](docs/control-flow.md) — target event, state, task,
  review-loop, and recovery flow.
- [`docs/adr/0001-rust-control-plane.md`](docs/adr/0001-rust-control-plane.md) —
  decision to implement the target runtime in Rust.
- [`docs/adr/0002-authoritative-ledger.md`](docs/adr/0002-authoritative-ledger.md) —
  decision that the control-plane ledger is authoritative and Hermes is the
  execution queue and operator UI.
- [`docs/runbooks/deployment.md`](docs/runbooks/deployment.md) — target build,
  provenance, install, rollback, and canary-activation contract.
- [`docs/migration-roadmap.md`](docs/migration-roadmap.md) — incremental Python
  reference-to-Rust migration with exit criteria.
- [`docs/implementation-status.md`](docs/implementation-status.md) — current
  executable inventory, inert boundaries, and remaining cutover work.
- [`docs/worker-result-contracts.md`](docs/worker-result-contracts.md) — exact
  versioned JSON returned by each worker role.

## Target roles

Role names express responsibilities, not implementation language or provider
marketing names. Exact provider/model bindings live in versioned policy.

| Role | Responsibility |
|---|---|
| `planner` | Validate the issue and root cause; produce an authorized plan. |
| `builder` | Implement the active plan, add regression coverage, and create/update a draft PR. |
| `reviewer-general` | Independently review correctness, integration, tests, and maintainability. |
| `reviewer-secperf` | Independently review security, privacy, concurrency, and performance. |
| `final-reviewer` | Reconstruct the complete case and decide its next disposition. |

Every reasoning run starts in a fresh agent session. The deterministic Rust
engine owns workflow state and never uses an LLM to choose transitions.

## Target system boundary

```text
GitHub webhook/reconciler
          |
          v
authoritative Rust case ledger
          |
          v
deterministic transition engine
          |
          v
repository Hermes board
          |
          v
fresh planner/builder/reviewer task
          |
          +---- validated immutable result ----> ledger
```

Hermes Kanban is the repository-scoped queue and operational view. It is not a
second workflow database. A Kanban completion cannot release downstream work
until the control plane has independently validated and committed it.

## Canary policy

The engine must not contain a canary issue number, comment ID, PR number,
personal notification destination, or repository-specific scope rule.

The MDK canary is constrained by policy instead:

- one configured repository and board;
- `pip-ok` applied by a trusted actor;
- intake and dispatch explicitly enabled;
- at most one active case;
- only one issue deliberately carries the label during the trial;
- shadow disposition and human merge only;
- no autonomous merge until JG explicitly changes policy.

Removing authorization, taking over the PR, pausing the repository, or changing
the exact reviewed head fails closed.

## Repository layout during migration

```text
docs/                   Canonical architecture, ADRs, migration, and runbooks
skills/                 Canonical shared and role-specific skills
schemas/                Legacy JSON contracts retained as migration inputs
manifests/              Legacy role manifests retained as migration inputs
src/pip_agent/          Legacy Python reference implementation
tests/                  Legacy behavioral and safety tests
scripts/                Rust release/lifecycle tools plus retained legacy installer
migration/               Language-neutral Python-to-Rust compatibility fixtures
crates/                  Rust core, contracts, store, adapters, controller, and CLI
```

Canonical skills remain under `skills/`; runtime profile directories must
symlink to their installed, content-addressed copies.

## Legacy prototype diagnostics

These commands inspect the Python reference implementation. They are not a
production deployment procedure.

```bash
uv run --locked pip-v2-bootstrap --repo-root "$PWD"
uv run --locked pip-v2-fixture
uv run --locked --dev pytest -q
uv build --wheel
```

Do not run `scripts/install-control-plane.sh` from the current migration branch.
The legacy installer and runtime remain issue-specific and do not have
cryptographically bound source provenance.

The Rust workspace and disposable systemd lifecycle are checked with:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --locked
scripts/test-systemd-lifecycle.sh
```

## Non-negotiable invariants

- Orchestration is deterministic and token-free.
- Models are explicitly pinned; substitution blocks the run.
- CI and both mandatory reviews bind to the exact PR head.
- Completed runs and accepted external evidence are append-only.
- Builders never broaden scope or edit another repository opportunistically.
- Credentials never enter this repository, prompts, task bodies, logs, or run
  artifacts.
- MDK remains in shadow merge mode until JG explicitly changes it.
- A release artifact must be cryptographically bound to its reviewed source
  and immutable manifest before installation.
