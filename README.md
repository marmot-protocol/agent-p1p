# agent-p1p

`agent-p1p` is the source repository for Pip: a repository-scoped,
deterministic control plane that turns authorized GitHub issues into planned,
built, independently reviewed, and finally adjudicated pull requests through
Hermes Kanban boards.

## Status

Pip is undergoing a lean Rust refactor before completing its first live
end-to-end shadow issue. Planning has run, but a complete builder/reviewer/final
review pipeline and human-ready PR are not yet proven.

The target keeps signed webhooks, an authoritative Rust workflow ledger,
unmodified Hermes and a narrow Cursor adapter. Operational failures must be
isolated from issue-work failures, and saved jobs must survive upgrades.
Automatic merge is deferred; the MDK pilot remains human-merge-only.

See [implementation status](docs/implementation-status.md) for the current
refactor inventory and dated host evidence. Python is a frozen migration
reference, not an installable runtime; it will be removed after cutover proof.


## Documentation

- [`docs/pip-architecture-plan.md`](docs/pip-architecture-plan.md) —
  canonical target architecture and invariants.
- [`docs/current-python-canary.md`](docs/current-python-canary.md) — accurate
  inventory of the legacy Python prototype and its gaps.
- [`docs/control-flow.md`](docs/control-flow.md) — workflow and source map,
  review-loop, and recovery flow.
- [`docs/adr/0001-rust-control-plane.md`](docs/adr/0001-rust-control-plane.md) —
  decision to implement the target runtime in Rust.
- [`docs/adr/0002-authoritative-ledger.md`](docs/adr/0002-authoritative-ledger.md) —
  decision that the control-plane ledger is authoritative and executor queues
  are projections of committed intent.
- [`docs/runbooks/deployment.md`](docs/runbooks/deployment.md) — target build,
  provenance, install, rollback, and canary-activation contract.
- [`docs/migration-roadmap.md`](docs/migration-roadmap.md) — remaining cutover,
  simplification and Python-retirement gates.
- [`docs/implementation-status.md`](docs/implementation-status.md) — current
  executable inventory, inert boundaries, and remaining cutover work.
- [`docs/completion-audit.md`](docs/completion-audit.md) — current live-workflow
  and lean-architecture completion gates, with explicit remaining gaps.
- [`docs/worker-result-contracts.md`](docs/worker-result-contracts.md) — exact
  versioned JSON returned by each worker role.

## Target roles

Role names express responsibilities, not implementation language or provider
marketing names. Exact provider/model bindings live in versioned policy.
Review roles are semantic publication lanes. Policy may define multiple stable
reviewer instances in either lane and mark each `required`, `advisory`, or
detached `shadow`; only required instances participate in workflow decisions.

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
          +-----------------------+
          |                       |
          v                       v
repository Hermes board   durable Rust direct-job queue
          |                       |
          v                       v
fresh Hermes-native task  fresh Cursor task
          |                       |
          +--- validated immutable result ---> ledger
```

Hermes Kanban is the repository-scoped queue and operational view for
Hermes-native roles. Direct-provider jobs use the ledger's durable queue and
may later be mirrored to the board for visibility, but Hermes never executes
them. The direct queue crosses a controller-owned, immutable inbox/result-file
boundary into a separate `pip-worker` identity that cannot open the ledger,
Hermes state, repository cache, or systemd credentials. Neither queue is a
second workflow database: no completion can release downstream work until the
control plane validates and commits it.

## Inspecting durable state

Use the installed executable with the ledger owner's privileges:

```sh
sudo -u pip-control /opt/pip/current/bin/pip-control status --database /var/lib/pip/ledger.db
sudo -u pip-control /opt/pip/current/bin/pip-control status --database /var/lib/pip/ledger.db --case 'CASE_KEY'
sudo -u pip-control /opt/pip/current/bin/pip-control status --database /var/lib/pip/ledger.db --attempt ATTEMPT_ID
```

The selectors are available in source after `c1f70e7`; check the installed release
before use. They return the exact case/history or attempt/error without writes,
provider calls or GitHub credentials. Attempt state is the ledger's observation,
not proof that an operating-system process is currently alive. Inspect systemd
and the retained attempt logs when an execution outcome is uncertain.

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
uv run --locked pip-bootstrap --repo-root "$PWD"
uv run --locked pip-fixture
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
