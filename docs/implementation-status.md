# Rust implementation status

**Snapshot date:** 2026-08-20
**Activation state:** no live Rust intake or dispatch is authorized

This file is the implementation inventory. The target behavior remains defined
by [`pip-v2-architecture-plan.md`](pip-v2-architecture-plan.md); the migration
exit gates remain defined by [`migration-roadmap.md`](migration-roadmap.md).
An entry is `implemented` only when an executable path and its local tests
exist. It is not evidence of live-host installation or a completed canary.

## What exists

| Boundary | Current implementation | Evidence boundary |
|---|---|---|
| Deterministic workflow | Exhaustive Rust states, events, effects, exact-head joins, bounded remediation, and shadow-only MDK disposition | Workspace tests and frozen fixtures |
| Authoritative storage | SQLite schema v3, immutable events/evidence/runs/findings, current-case projection, durable outbox, leases, transactional effect supersession, backup, migration, and crash injection | Workspace and disposable lifecycle tests |
| Intake reads | Generic label discovery using numeric repository/actor identity, policy validation, bounded pagination, and one-case concurrency | Fixture tests plus a read-only live GitHub observation |
| Hermes reads/projection | Bounded CLI adapter, capability/task/run reads, controller-owned gates, idempotent projections, result metadata ingestion, and restart convergence | Fake-runner and offline integration tests only |
| Worker contracts | Versioned planner, builder, two reviewer, and final-reviewer results bound to case, task, role, model, skills commit, plan, PR, and exact head | Contract fixtures and ingestion tests |
| CI reconciliation | Independent current and historical check/status evaluation on the ledger-bound PR head | Fixture and controller-cycle tests |
| GitHub writes | Idempotent issue comment, draft PR, exact-head review, and guarded merge adapter primitives | Adapter tests; not all are wired to outbox consumption |
| Release/install | Signed source-bound release cohort, artifact verification, content-addressed install, rollback, schema migration, and hardened non-dispatching timer | Local tests and disposable systemd container |
| Active controller | One `controller-cycle` command that ingests completed runs, reconciles CI, performs generic intake, commits authorization removal or foreign-PR takeover, and projects dispatch effects only while fresh gates pass | Local tests; active unit template is packaged but not installed or enabled |

## What is deliberately inert

- `config/target/repositories/mdk.json` has intake disabled, repository paused,
  dispatch disabled, merge mode `shadow`, and autonomous merge false.
- The production installer installs only the non-dispatching shadow reconciler.
- `pip-v2-controller@.service` and its timer are staged release resources. The
  installer does not copy, enable, or start them.
- No Rust process has created an MDK task, branch, comment, PR, review,
  notification, or merge in a live environment.

## Remaining cutover work

These are implementation gaps, not merely missing operational evidence:

1. Consume every durable GitHub effect through the Rust controller. In
   particular, independently verify and publish planner comments, case-owned
   branches/draft PRs, role-stamped reviews, and human-held disposition without
   giving workers direct workflow authority.
2. Build the final-review preflight for review threads, current mergeability,
   policy revision, and exact published review evidence. Authorization removal
   and foreign PR ownership/head changes now commit immutable abandonment or
   takeover events and transactionally supersede older pending effects.
3. Build the complete deterministic final-review bundle from immutable ledger
   references and fresh GitHub observations. A valid final worker result alone
   is not sufficient.
4. Provision and test a compatible Hermes runtime under the dedicated
   `pip-v2-control` identity, including repository board, profiles, exact skill
   links, shared authentication, provider probes, and the gateway/dispatcher
   observing the same Hermes root.
5. Add active-unit installation, upgrade, rollback, and restart tests while
   preserving the rule that a fresh install remains disabled.
6. Record live Hermes and provider outage/recovery evidence, then seek explicit
   authorization for one MDK shadow case.

## Runtime topology decision

The active controller must not depend on an operator's personal
`~/.hermes`. It uses a service-owned root:

```text
pip-v2-control system identity
  /var/lib/pip-v2/hermes
    Kanban database and board state
    managed profiles and canonical skill links
    shared authentication links provisioned outside the release
```

Both `HERMES_HOME` and `HERMES_KANBAN_HOME` are set to that root by the staged
unit. The compatible Hermes gateway/dispatcher must run against the same root,
because it is the component that claims ready tasks. This topology is based on
Hermes' documented `HERMES_HOME`, Kanban storage override, and board dispatcher
behavior; it still requires a real-host compatibility and recovery test before
activation.

Upstream references:

- [Hermes CLI command reference](https://github.com/nousresearch/hermes-agent/blob/main/website/docs/reference/cli-commands.md)
- [Hermes Kanban guide](https://github.com/nousresearch/hermes-agent/blob/main/website/docs/user-guide/features/kanban.md)
- [Hermes environment layout](https://github.com/NousResearch/hermes-agent/blob/main/AGENTS.md)
