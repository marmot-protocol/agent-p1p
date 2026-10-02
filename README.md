# agent-p1p

`agent-p1p` is the source repository for Pip: a repository-scoped,
deterministic control plane that turns authorized GitHub issues into planned,
built, independently reviewed, and finally adjudicated pull requests through
Hermes Kanban boards.

## Status

Pip has taken one MDK issue to a human-held, ready-for-review PR. The October
2026 reliability pass changed how it handles failure: transient problems back
off, work failures retry per stage, and anything else pauses the case with an
explanation that a maintainer resolves from GitHub. See [status](docs/status.md)
for what is deployed and what still needs live proof.

## When Pip pauses

Pip posts a comment on the issue explaining what stopped. A trusted maintainer
replies with one of:

```text
@agent-p1p resume      continue where it stopped (guidance on following lines)
@agent-p1p replan      start again from planning
@agent-p1p abandon     stop; re-adding the label restarts it
```

See [parked cases](docs/runbooks/parked-cases.md).

## Documentation

- [`docs/pip-architecture-plan.md`](docs/pip-architecture-plan.md): canonical
  architecture and invariants.
- [`docs/status.md`](docs/status.md): what is deployed, what still needs live
  proof, remaining work.
- [`docs/control-flow.md`](docs/control-flow.md): workflow and source map.
- [`docs/runbooks/parked-cases.md`](docs/runbooks/parked-cases.md): why cases
  pause, budgets, and the resume commands.
- [`docs/runbooks/deployment.md`](docs/runbooks/deployment.md): build,
  provenance, install, rollback and rollout.
- [`docs/github-conversations.md`](docs/github-conversations.md): mentions and
  human feedback.
- [`docs/adr/`](docs/adr/): the Rust control plane and authoritative ledger
  decisions.
- [`docs/worker-result-contracts.md`](docs/worker-result-contracts.md): what
  each worker role returns.

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

## Repository layout

```text
crates/      Rust core, contracts, store, adapters, controller and CLI
skills/      Canonical shared and role-specific worker skills
docs/        Architecture, status, ADRs, runbooks and historical evidence
config/      Repository policy (target and activation revisions)
packaging/   systemd units
scripts/     Release, install and lifecycle tooling
tests/       Shell checks, lifecycle harness and probe fixtures
migration/   Frozen parity fixtures used by Rust tests
```

Canonical skills remain under `skills/`; runtime profile directories must
symlink to their installed, content-addressed copies.

## Checks

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
