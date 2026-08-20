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
| Authoritative storage | SQLite schema v3, immutable events/evidence/runs/findings, current-case projection, durable outbox, leases, transactional effect supersession, atomic direct-job enqueue, backup, migration, and crash injection | Workspace and disposable lifecycle tests |
| Intake reads | Generic label discovery plus bounded issue title/body/comment and label-event snapshots using numeric repository/actor identity, policy validation, and one-case concurrency | Fixture tests plus a read-only live GitHub observation |
| Worker dispatch routing | Hermes-native roles receive controller-gated, idempotent board projections; direct roles become leased `RUN_DIRECT_WORKER` outbox jobs in the same transaction that acknowledges the source dispatch; mixed reviews split one role to each path; direct Cursor and untyped paths fail before a Hermes subprocess | Fake-runner, injected-failure, mixed-review, and offline integration tests; the direct-job consumer and workspace allocation remain below |
| Worker contracts | Versioned planner, builder, two reviewer, and final-reviewer results bound to case, task, role, model, skills commit, plan, PR, and exact head | Contract fixtures and ingestion tests |
| Worker evidence bundles | Every projected worker receives the complete ordered ledger history at the claimed state revision, including record digests and a reproducible root digest; final review includes the atomically committed GitHub preflight | Ledger, scheduling, dispatch-command, and exact-final-preflight tests |
| CI reconciliation | Independent current and historical check/status evaluation on the ledger-bound PR head | Fixture and controller-cycle tests |
| GitHub reads/writes | Bounded REST reads plus bounded GraphQL review-thread pagination; idempotent issue comments, controller-owned draft PRs, exact-head reviews, ready-for-review mutation, and guarded merge | Adapter and controller-cycle tests; MDK policy cannot enable the guarded path |
| Release/install | Signed source-bound release cohort, artifact verification, content-addressed install, rollback, schema migration, hardened shadow timer, and inert active-controller templates | Local tests and disposable systemd container |
| Active controller | One `controller-cycle` command that ingests completed Hermes runs, reconciles CI, performs generic intake, commits authorization removal or foreign-PR takeover, publishes plans and role reviews, verifies the final-review preflight, publishes human-held disposition comments, records local terminal effects, and routes dispatch effects only while fresh gates pass | Local fixture tests; not executable end to end until direct-job consumption, exact workspace allocation, and Hermes bootstrap below are implemented |
| Final-review preflight | A durable observation effect joins the accepted plan/build/reviewer ledger, fresh issue/clarification and authorization evidence, exact numeric GitHub actor and role-stamped approvals, current head CI, clean draft-PR ownership/mergeability, and resolved review threads before final-review dispatch | State-machine, adapter, fixture, drift, and restart-safe lease tests |
| Review publication | Two distinct controller-held reviewer credentials publish the joined role contracts on the exact head; remediation and final preflight remain blocked until both idempotent reviews exist | Policy, state-machine, mutation, outage/retry, and exact-role fixture tests |
| Plan publication | Planner results first create a durable `PUBLISH_PLAN` effect; the controller publishes the immutable plan comment and only then applies the typed outcome that releases build, human disposition, or terminal recording | Contract, state-machine, mutation, and outage/retry tests |
| Draft PR publication | A review-ready builder result records only its clean local commit; after verified controller branch publication, the same durable effect creates or updates the stable case-owned draft PR and only then binds PR/head and releases independent CI observation | Initial/remediation identity, branch/PR outage retry, state-machine, and exact-head tests |
| Branch publication | Builder tasks receive deterministic case-owned worktree and branch assignments but no GitHub credential; the draft-publication effect canonicalizes the worktree under the controller root, pins and directly uses the sole push URL, disables repository hooks/filesystem monitors/credential helpers/proxies/HTTP headers, forces TLS verification, validates clean local branch/head state, uses exact force-with-lease against the ledger's prior remote head, verifies the remote SHA, and only then mutates the draft PR | Assignment, scope, URL-drift, multiple-push-URL, real-bare-remote, race, retry, and controller-cycle tests |
| Guarded merge | An explicitly guarded/autonomous policy selects the merge method; the controller revalidates the complete final gate, marks the draft ready, revalidates, emits a separate merge effect, merges with expected-head protection, and verifies the recorded merge commit | Restart-convergence, shadow-disablement, state-machine, GraphQL, and mutation tests |
| Human disposition | `HOLD_FOR_HUMAN`, `ESCALATE`, and shadow-ready effects publish idempotent provenance-marked issue or draft-PR comments; local completion, block, abandonment, and takeover effects commit evidence without writing after lost authorization | Mutation fixtures and transactional effect/evidence tests |

## What is deliberately inert

- `config/target/repositories/mdk.json` has intake disabled, repository paused,
  dispatch disabled, merge mode `shadow`, and autonomous merge false.
- The production installer does not enable or start either reconciliation path.
- `pip-v2-controller@.service` and its timer are installed as templates. The
  installer does not enable or start an instance.
- No Rust process has created an MDK task, branch, comment, PR, review,
  notification, or merge in a live environment.

## Remaining cutover work

These are implementation gaps, not merely missing operational evidence:

1. Implement the `RUN_DIRECT_WORKER` consumer: validate the stored immutable
   job, obtain a fresh exact-model provider probe, invoke `CursorExecutor`, and
   feed its bound result into the same ingestion/state-machine path used by
   Hermes results. Failed attempts must remain leased/retryable and must not
   advance workflow state.
2. Add a policy-bound canonical repository checkout location, reconcile that
   checkout, and allocate exact case/head worktrees before dispatch. Then pass
   only allocated paths to either executor rather than the configured root, and
   emit Hermes' typed `dir:<absolute-path>` or `worktree:<absolute-path>`
   workspace contract. The scheduler now rejects relative roots and emits a
   typed path, but it still points at the configured workspace root because the
   tested Rust allocator is not connected.
3. Provision and test a compatible Hermes runtime under the dedicated
   `pip-v2-control` identity, including repository board, profiles, exact skill
   links, shared authentication, the controller-only Git credential helper,
   provider probes, and the gateway/dispatcher observing the same Hermes root.
4. Record live Hermes and provider outage/recovery evidence, then seek explicit
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
unit. The compatible Hermes gateway/dispatcher must run against the same root
and may claim only Hermes-native role tasks. Direct Cursor work is now queued as
leased ledger outbox work and must never be represented as a Hermes provider
override; its service consumer is not implemented yet. This topology is based
on Hermes' documented profile,
`HERMES_HOME`, Kanban storage, typed workspace, and dispatcher behavior; it
still requires implementation plus a real-host compatibility and recovery test
before activation.

Upstream references:

- [Hermes CLI command reference](https://github.com/nousresearch/hermes-agent/blob/main/website/docs/reference/cli-commands.md)
- [Hermes Kanban guide](https://github.com/nousresearch/hermes-agent/blob/main/website/docs/user-guide/features/kanban.md)
- [Hermes profiles guide](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/profiles.md)
- [Hermes environment layout](https://github.com/NousResearch/hermes-agent/blob/main/AGENTS.md)
- [GitHub GraphQL pull-request and review-thread schema](https://docs.github.com/en/graphql/reference/pulls)
- [GitHub review rules, including the self-approval prohibition](https://docs.github.com/en/pull-requests/how-tos/review-pull-requests/approving-a-pull-request-with-required-reviews)
