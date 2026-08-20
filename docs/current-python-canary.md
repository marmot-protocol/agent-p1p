# Legacy Python canary inventory

**Snapshot:** Migration baseline `2607004` on 2026-08-20

**Purpose:** Describe what exists, not what the Rust target promises

The Python code is a substantial single-issue safety prototype. It contains
useful contracts, state-machine ideas, evidence validation, and deployment
hardening. It is not a generic repository workflow engine and must not be
treated as the target architecture.

## Baseline health

- The worktree was clean at the baseline commit.
- GitHub CI was red with three Linux failures.
- The test suite collected 248 tests.
- Profile dry-run, offline shadow fixture, wheel build, and `bash -n` passed.
- A macOS run also encountered eight AF_UNIX path-length failures caused by long
  pytest temporary paths; these were separate from the three Linux failures.
- The wheel did not contain a source-commit binding. The installer accepted an
  operator-supplied SHA and wrote it beside the installed wheel digest.

Do not install the migration baseline.

## Implemented components

| Component | Legacy module | What it provides |
|---|---|---|
| Role/profile bootstrap | `bootstrap.py`, manifests, skills | Hermes profile creation, pinned models/toolsets, skill/auth symlinks, board creation. |
| Worker contracts | `contracts.py`, `schemas/` | JSON Schema validation, model mismatch rejection, exact-head join checks. |
| Direct Cursor process adapter | `cursor_adapter.py` | Fresh invocation, bounded output, secret canaries, task binding, reviewer worktree mutation check. |
| Permanent-case primitives | `case_store.py`, `state_machine.py` | SQLite cases/events/runs, append-only guards, bounded transition helpers. |
| Generic-looking intake prototype | `intake.py` | Label/timeline validation and one planner task command, still restricted to an exact canary configuration. |
| Canary decision reconciliation | `decision_reconciler.py` | Fixed issue/comment parsing, trusted numeric identities, planner/human decision binding. |
| GitHub final gate | `github_gate.py` | Exact-head PR/CI/review/mergeability validation for MDK. |
| Hermes routing/gates | `kanban_router.py`, `kanban_gate.py`, `route_consumer.py` | Fixed canary task graph, semantic gates, live authorization rechecks, held dispositions. |
| Offline reference flow | `offline_fixture.py` | In-process planner-to-shadow-ready demonstration using SQLite. |
| Installation | `install-control-plane.sh` | Root-owned wheel deployment, system identity, units/timers, snapshots, rollback. |

## Actual deployed flow

```text
fixed MDK issue/comments
        |
decision reconciler timer
        |
SQLite planning/human-decision state + route JSON
        |
route-consumer timer running as the Hermes owner
        |
fixed Hermes DAG and activation gates
        |
worker result metadata retained by Hermes
```

The control socket exposes only `ensure_canary` and `status`. The decision
reconciler mutates planning/human-decision state. The route consumer validates
build, review, remediation, and final results held in Hermes.

It does **not** append those deployed worker results to SQLite or advance the
SQLite case through `REVIEWING`, `FINAL_REVIEW`, and `SHADOW_READY`. That full
ledger path exists only in the offline fixture. Consequently, SQLite and Hermes
are overlapping partial workflow representations rather than one recoverable
authority.

## Hard-coded canary behavior

The legacy runtime embeds values including:

- `marmot-protocol/mdk` and issue `1240`;
- issue, comments, default-branch, and PR URLs;
- historical planner comment IDs/digests and a planned base;
- trusted actor logins and numeric IDs;
- notification destination identifiers;
- the `pip-mdk` board;
- a permanently ineligible PR number;
- sensitive-scope keyword heuristics; and
- a compiled DAG revision.

The repository and board JSON files do not drive the installed decision and
routing path. They are partly policy documentation and partly inputs to the
separate intake prototype.

## Fixed workflow shape

The router materializes seven worker tasks plus activation gates and a sticky
sentinel:

```text
build
  -> general review 1 + security/performance review 1
  -> optional remediation
  -> general review 2 + security/performance review 2
  -> final review
  -> human-held disposition
```

This is not the target convergence loop. A blocker in the second review round
creates a human-held task instead of another bounded remediation round.

## Baseline CI failures

The migration baseline has three Ubuntu failures:

1. A route-upgrade test calls `preserve_dag_revision`, which the archiving
   implementation does not accept.
2. Two Kanban metadata tests disagree with the implementation about whether
   the artifact/session envelope is returned after contract validation.

These failures must be resolved or intentionally superseded before using the
Python baseline as a parity oracle.

## Useful assets to preserve

- State transition and fail-closed test cases.
- JSON schemas and representative valid/invalid fixtures.
- Numeric actor and evidence-digest checks.
- Exact-head CI/review join rules.
- Model/task/skill binding checks.
- Secret-output and reviewer-mutation tests.
- Installer snapshot, content-addressing, and rollback threat analysis.
- Role skills and their independent review rubrics.

Preservation means porting the behavior or retaining a fixture with an explicit
reason. It does not mean mechanically translating every Python module.

## Behavior not yet implemented

- Generic multi-issue or multi-repository operation.
- One authoritative deployed case ledger.
- Dynamic remediation beyond one fixed round.
- Deterministic worktree/branch/PR lifecycle management.
- Signed webhooks and delivery ledger.
- Cross-repository dependency orchestration.
- Provider health, quota state, cooldown, and recovery probes.
- Global pause/concurrency/reporting.
- Guarded merge.
- Cryptographically source-bound releases.
- Checked-in disposable-systemd lifecycle tests.
- Hermes/Cursor version and capability compatibility policy.

## Retirement rule

Do not delete or rewrite the Python prototype in place. Freeze it as a reference
while Rust fixtures reach parity. Stop and archive the Python production service
only after the Rust shadow reconciler and disposable-systemd lifecycle gates
pass. Retain the final source commit, wheel digest, database snapshot, installed
unit snapshot, and rollback instructions.
