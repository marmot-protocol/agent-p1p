# Pip v2 target architecture

**Status:** Approved direction; Rust core migration in progress

**Scope:** Full multi-repository target, beginning with an MDK shadow canary

**Audience:** JG, Pip maintainers, operators, and future implementers

This is the canonical target architecture required by `AGENTS.md`. It defines
the system we intend to build, not the behavior of the legacy Python canary.
See [`current-python-canary.md`](current-python-canary.md) for current reality
and [`migration-roadmap.md`](migration-roadmap.md) for the transition.

The MDK pilot remains shadow and human-merge-only until JG explicitly changes
that policy. This document does not authorize production activation or merge.

## 1. Goal

Build a durable control plane that:

1. discovers explicitly authorized GitHub issues;
2. validates the issue and its root cause before implementation;
3. produces a versioned, human-readable and machine-consumable plan;
4. creates or updates one Pip-owned draft PR;
5. requires two independent reviews of the same exact PR head;
6. repeats remediation and same-head review until convergence or escalation;
7. performs a fresh holistic final review of the complete case;
8. stops at a human-held recommendation in shadow mode;
9. optionally performs a guarded merge only when repository policy explicitly
   permits it; and
10. preserves enough immutable evidence to reconstruct every decision.

One configurable engine serves all repository boards. Canary limitations are
policy, not special-purpose code.

## 2. Non-negotiable principles

- **Deterministic orchestration.** Models reason about issues and code; Rust
  code validates evidence and chooses transitions.
- **One authoritative ledger.** The control-plane database owns case state,
  accepted evidence, immutable runs, and dispatch intent.
- **Executor queues are projections.** Hermes presents and executes
  Hermes-native work; the ledger queues direct-provider work. Neither is a
  second workflow database.
- **Plan before implementation.** The builder never invents missing product or
  protocol intent.
- **Fresh reasoning sessions.** Durable context comes from case artifacts, not
  prior model conversation state.
- **Exact-head evidence.** CI and reviews are valid only for the PR head they
  examined.
- **No silent model fallback.** A missing or substituted model blocks the run.
- **Repository scope is a boundary.** A worker cannot opportunistically edit a
  dependency repository.
- **Append-only history.** Accepted external evidence and completed runs are
  never edited in place.
- **Least privilege.** Worker capabilities match their role. Merge authority is
  separate from planning, building, and review.
- **Fail closed, recover deterministically.** Ambiguous, stale, missing, or
  conflicting evidence cannot release downstream work.
- **No embedded pilot identity.** Repository, issue, actor, notification, PR,
  and branch values come from validated policy and live evidence.

## 3. System context

```text
                         GLOBAL POLICY AND HEALTH
              repository registry / provider capacity / pause
                                  |
               +------------------+------------------+
               |                  |                  |
            pip-mdk          pip-goggles       pip-client-...
               |                  |                  |
               +------ repository-scoped cases -----+
                                  |
                                  v
                      AUTHORITATIVE RUST LEDGER
                       events / cases / runs
                                  |
                                  v
                    DETERMINISTIC WORKFLOW ENGINE
                                  |
                       +----------+------------------+
                       |          |                  |
                  GitHub adapter  |             Hermes adapter
                                  |                  |
                          direct-job queue     repository board
                                  |                  |
                                  +--- fresh role workers
```

The global layer owns only cross-board policy and resources. It does not
interpret code, summarize away evidence, override a verdict, or select a
different model.

## 4. Rust workspace boundary

The target implementation is a Rust workspace with dependency direction toward
a pure core:

```text
crates/
  pip-core/       domain types, state machine, policy, transition decisions
  pip-contracts/  versioned worker and evidence contracts
  pip-store/      SQLite events, cases, immutable runs, dispatch outbox
  pip-github/     authenticated GitHub reads/writes and webhook verification
  pip-hermes/     board capability probe and task/result adapter
  pip-executor/   direct provider process adapters and worktree bindings
  pip-control/    daemon, reconciler, CLI, health, and systemd entry points
```

`pip-core` must not depend on network, subprocess, wall-clock, filesystem,
SQLite, Hermes, GitHub, or model-provider code. Tests provide explicit events,
policy, and time. The core returns transition decisions and required effects.

Adapters perform effects and return attributable evidence. An outbox records
the intent before any external side effect so restart recovery is idempotent.

Every worker projection contains an `immutable_evidence_bundle` produced from
the authoritative ledger at the outbox effect's exact case revision. Version 1
contains deterministically ordered events, accepted worker runs, controller
evidence, and findings, including each stored payload digest. The controller
rejects a stale history and caps the encoded bundle at 512 KiB. Its root digest
is SHA-256 over compact, lexicographically key-ordered JSON after removing only
the top-level digest field. This makes the complete accepted ledger history
self-contained and reconstructable without granting a worker ledger access.

## 5. Boards, repositories, and cases

Each watched repository has one Hermes board and one repository policy. A board
contains task projections for cases owned by that repository.

A permanent case identity is `(repository_id, issue_number, workflow_version)`.
The ledger records:

- current state and state revision;
- active plan version and authorization evidence;
- branch, worktree, PR, and exact head;
- build, review, remediation, and final-review rounds;
- active and resolved findings;
- dependencies and human decisions;
- provider/model and skill versions;
- dispatch outbox entries and Hermes task projections;
- append-only events and immutable run payloads.

Hermes task status is observed evidence. It does not directly mutate case state.
The engine validates a completed result, commits the run and transition, and
then publishes the next task projection.

## 6. Repository policy

Policy is versioned, schema-validated, and installed separately from secrets.
It includes:

- repository and default branch;
- Hermes board;
- intake label and trusted label actors;
- issue exclusions and human-held work;
- enabled roles and exact provider/model/reasoning bindings;
- required CI and mergeability rules;
- branch ownership rules;
- sensitive-scope escalation categories;
- loop, elapsed-time, concurrency, and retry bounds;
- shadow or guarded-merge disposition;
- notification destination references, not credentials;
- global and repository pause state.

Changing policy creates a new policy revision. An active case retains its
accepted revision unless a change is explicitly declared immediately
restrictive, such as pause, authorization removal, or merge disablement.

## 7. Intake and authorization

Primary intake comes from signed GitHub webhooks. A bounded periodic reconciler
recovers missed deliveries. Delivery IDs and canonical event fingerprints are
idempotency keys.

An issue becomes eligible only when:

- it is open and is not a pull request;
- the configured label is currently present;
- the latest relevant label event came from a trusted numeric GitHub actor;
- it is not excluded, held, duplicated, or already owned by another workflow;
- repository intake is enabled and global/repository pause is clear; and
- repository and global active-case limits allow it.

The controller creates the case before it dispatches a planner. Losing
authorization prevents new worker activation and moves active work to a durable
hold according to policy.

## 8. Roles and execution

| Role | Target responsibility | Write authority |
|---|---|---|
| `planner` | Validate issue, root cause, scope, dependencies, and test plan. | Versioned plan artifacts and a bound result contract only. |
| `builder` | Manage the assigned worktree, implement the active plan, test, and create the exact local commit. | Assigned worktree/branch only; no GitHub credential or mutation. |
| `reviewer-general` | Correctness, integration, errors, concurrency, tests, maintenance. | Review evidence/comments only. |
| `reviewer-secperf` | Security, privacy, authorization, abuse, resource bounds, performance. | Review evidence/comments only. |
| `final-reviewer` | Reconstruct the complete case and determine the next disposition. | Final evidence/comment only. |

Write authority is exercised by deterministic controller adapters using
role-scoped credentials; model processes return contracts and never receive
GitHub tokens. The PR-author identity and the two reviewer identities must be
three distinct numeric actors. This is required because GitHub forbids a pull
request author from approving that pull request and required approval counts
represent reviewers, not multiple personas of one account.

Exact models are policy values rather than role names. A provider adapter must:

1. probe the required model and authentication path;
2. render a complete prompt from immutable inputs and versioned skills;
3. start a fresh session in the assigned workspace;
4. capture bounded output and durable artifacts;
5. record requested and reported model identity;
6. reject mismatch or unverifiable required fields; and
7. return a versioned contract without choosing a workflow transition.

Provider-side model routing may not be cryptographically attestable. The run
must record the precise assurance available rather than claim more.

Execution mode is also a policy value and selects an executor, not merely a
task-body annotation:

- `hermes` roles are projected to the repository board with an existing Hermes
  profile and Hermes-supported exact provider/model override. Their workspace
  is encoded as `dir:<absolute-path>` or `worktree:<absolute-path>`.
- `direct` roles are committed to a durable controller queue and consumed by
  the Rust direct-provider service. The board may show a controller-owned
  mirror for operator visibility, but the Hermes gateway cannot claim or run
  it, and `cursor` is never passed to Hermes as a provider.
- Both paths persist the same immutable binding, artifacts, result contract,
  lease/attempt history, and typed terminal outcome before the state machine can
  advance.

The controller allocates or reconciles the repository checkout and exact
case/head worktree before either executor receives a job. A workspace-root path
is never treated as an executable task workspace.

## 9. Planning contract

The planner establishes:

- whether the behavior is real and still present on the current default branch;
- whether the issue describes the root cause or a symptom;
- whether another change already fixed or duplicated it;
- repository-local scope and explicit non-scope;
- cross-repository prerequisites;
- product, protocol, privacy, trust, persistence, UX, or API ambiguities;
- regression coverage and repository-native verification; and
- a planned-base SHA as evidence context, not a checkout lock.

Planner outcomes are typed. At minimum:

```text
PROCEED
ALREADY_FIXED
NOT_REPRODUCIBLE
DUPLICATE
ROOT_CAUSE_DIFFERENT_SCOPE
CROSS_REPO_DEPENDENCY
WAITING_FOR_ISSUE_CREATOR
NEEDS_HUMAN_SCOPE_DECISION
ABANDON
BLOCKED
```

`PROCEED` requires a complete repository-local plan with no unresolved decision.
Each plan version has a structured ledger record and immutable Markdown/JSON
artifact. The controller renders that result into a provenance-marked GitHub
comment using the PR-author credential. The result first records
`PLAN_RECORDED`; only successful idempotent publication applies its typed
planner outcome and releases builder, human-disposition, or terminal effects.
A trusted clarification creates another planning run; it never releases the
builder directly.

## 10. Build and draft-PR contract

The controller, not the model, assigns the repository, branch namespace,
worktree path, active plan, and expected base context. The builder:

1. verifies the plan still applies to the current default branch;
2. returns to planning only for a concrete incompatibility;
3. implements only the authorized scope;
4. adds regression coverage and runs required local checks;
5. inspects the full diff;
6. creates a Pip-attributed commit whose trust comes from the bound worker
   result and subsequent controller, CI, and review gates rather than Git
   author metadata;
7. creates that commit only on the assigned local Pip branch and leaves the
   assigned worktree clean;
8. reports the exact local commit without receiving a GitHub credential; and
9. leaves branch publication, draft-PR creation, and CI disposition to the
   controller.

The builder result first records `BUILD_RECORDED` and cannot release CI
observation. The same durable publication effect binds the accepted local head
to the deterministic case worktree and branch, rejects a dirty worktree or
branch/head drift, compares the current remote head with the ledger's prior
head, pushes only through exact `--force-with-lease`, and verifies the remote
SHA. Before any repository command, the controller canonicalizes the worktree
as a strict child of its configured root, pins the expected push URL, disables
repository hooks and filesystem monitors, and clears repository-provided
credential helpers, proxy settings, and HTTP headers while forcing TLS
verification and bounded redirect behavior. It then creates or updates one
draft PR using a stable case ownership marker, verifies the repository, branch,
author, base, and exact head returned by GitHub, and applies `REVIEW_READY` with
the PR/head binding. A crash after the push replays as an existing exact branch.
Remediation reuses the same marker and PR number while advancing only the
assigned branch head.

The engine independently reads GitHub before accepting those claims. Clean
default-branch movement does not force a rebase. A conflict, branch-protection
requirement, or material plan invalidation does. Any new head invalidates all
head-bound review and CI evidence.

## 11. Review convergence

After the accepted builder head has required green CI, the engine dispatches
both mandatory reviewers independently against that same SHA. Neither review
is a parent summary of the other.

Every blocking finding has a stable identity, origin role, reviewed head,
defect, consequence, corrective direction, and required resolution evidence.

If either reviewer requests changes:

1. the engine unions the mandatory findings;
2. dispatches one builder remediation run;
3. independently validates the new PR head and required CI;
4. invalidates both earlier approvals;
5. dispatches both reviewers again on the new exact head; and
6. requires the originating reviewer to confirm each applicable resolution.

This is a dynamic loop, not a pre-created fixed two-round DAG. Policy bounds
rounds, elapsed time, repeated finding fingerprints, and provider failures.
Crossing a bound creates a durable escalation; it never silently approves.

The exact-head join requires:

```text
current PR head = X
builder result and CI bind to X
general reviewer APPROVE binds to X
security/performance reviewer APPROVE binds to X
all mandatory findings are resolved and origin-confirmed at X
no blocking GitHub review or thread remains
Pip still owns the PR and branch
issue authorization remains valid
GitHub reports clean mergeability
```

The controller publishes and identifies the two mandatory GitHub reviews by an
exact, machine-readable line in the body: `Pip reviewer role:
reviewer-general` or `Pip reviewer role: reviewer-secperf`. The review actor
must be the configured numeric identity for that role, the review must approve
the current commit, and the latest same-role stamped review on that commit is
authoritative. The two role actors must differ from each other and from the PR
author. Worker result metadata alone cannot release final review.

Optional external review is advisory. Its concrete findings may enter the loop,
but absence, rate limiting, or failure never substitutes for a mandatory review.

## 12. Final review and disposition

The final reviewer receives immutable references to:

- the issue and authoritative clarifications;
- every plan and the active plan;
- dependency evidence;
- every build and remediation run;
- both complete review histories;
- finding resolutions and confirmations;
- the current diff, exact-head CI, and mergeability evidence; and
- authorization and ownership evidence.

Those references are delivered in the task's versioned evidence bundle. The
accepted `GITHUB_FINAL_PREFLIGHT` record includes a newly fetched original issue
title/body, all bounded issue comments with content digests, the current trusted
label event, and the exact PR/CI/review/thread observation. It is committed
atomically with the final-review dispatch effect, so the task cannot be released
from an earlier or partially observed join.

It asks whether the final PR solved the correct problem. Typed outcomes include:

```text
READY
RETURN_TO_BUILD
RETURN_TO_REVIEW
RETURN_TO_PLANNING
WAIT_FOR_HUMAN
BLOCKED
ABANDON
```

In shadow mode, `READY` becomes `SHADOW_READY` and produces a human-held
notification. It cannot merge. In explicitly authorized guarded-merge mode,
`READY` permits the deterministic merge transaction to begin.

For the repository-scoped implementation, the shadow notification is an
idempotent provenance-marked comment on the draft PR. Human-wait and escalation
notifications use the issue. Losing issue authorization suppresses new comment
writes; abandonment and takeover are still recorded locally and durably.

## 13. Guarded merge

Merge is a deterministic external transaction, never a free-form model action.
Immediately before merge it re-fetches and verifies:

- the current PR head and ownership;
- both mandatory exact-head approvals;
- all required CI attempts and required status contexts;
- absence of blocking reviews, threads, or new commits;
- current authorization and policy revision;
- clean mergeability; and
- final-review evidence bound to the same SHA.

Any change aborts. Because case PRs remain drafts throughout automated review,
the guarded transaction first uses GitHub's
`markPullRequestReadyForReview` mutation with the exact PR node identity, then
re-runs every gate against the non-draft PR. A separate durable
`EXECUTE_MERGE` effect performs the policy-selected merge method with GitHub's
expected-head SHA guard. Success records the merge commit and verifies GitHub
reports the PR merged. Restart after either external mutation converges by
observing the ready or merged PR; it does not issue a second logical
transaction. MDK remains shadow-only until JG explicitly changes its policy.

## 14. Global control plane

After the generic single-repository engine is proven, the global layer may own:

- repository/board registry and concurrency;
- provider authentication, model availability, quota state, and cooldown;
- dependency graph and duplicate-case prevention;
- global emergency pause and human-held PR registry;
- bounded recovery probes and retry suppression;
- aggregate audit and operational reporting.

Provider state is tracked per provider/model path. A failure pauses only roles
that require that path. Recovery requires a controlled successful probe and
emits one recovery event.

## 15. Security and trust boundaries

- The ledger directory is writable only by the control-plane service identity.
- Worker processes cannot open the ledger or control socket mutation API.
- GitHub credentials are role-scoped where practical and never copied into
  prompts or task bodies.
- Builders cannot merge; reviewers cannot push; the final reviewer cannot
  merge; the merge transaction cannot reason about code.
- Worktrees live under a controller-owned root and are assigned by exact path.
- Controller Git publication ignores worker-controlled hooks, filesystem
  monitors, credential helpers, proxies, and HTTP headers; requires the
  policy-bound push URL; and forces TLS verification before using its
  separately provisioned credential helper.
- All external payloads are size-bounded, schema-validated, and attributable.
- Accepted evidence records numeric actor/repository identity and immutable
  content digests.
- Technical host access does not imply workflow authorization.
- Human takeover freezes automation and suppresses future dispatch.

## 16. Release and provenance

A release consists of a Rust binary, installed skills/contracts, and an
immutable release manifest. The manifest binds at least:

- exact reviewed source commit;
- lockfile digest;
- binary and resource digests;
- target triple and compiler/toolchain identity;
- workflow/contract versions; and
- build identity and timestamp.

Trusted CI produces and signs the manifest. Installation verifies the signature
and every digest before mutation. An operator-supplied source SHA alone is not
provenance. See [`runbooks/deployment.md`](runbooks/deployment.md).

## 17. Canary activation

The generic MDK policy begins with:

```yaml
intake_enabled: false
dispatch_enabled: false
max_active_cases: 1
merge_mode: shadow
autonomous_merge: false
merge_method: squash
```

Activation requires a reviewed release and explicit operator action to enable
intake/dispatch. The operator deliberately places `pip-ok` on one suitable
issue and ensures no other issue is eligible. The engine discovers that issue
through the same generic path future cases will use.

There is no canary issue constant, pinned comment ID, special PR number, or
canary-specific DAG in the binary.

## 18. Verification

The target test ladder includes:

1. pure state-machine and policy tests;
2. contract and serialization compatibility tests;
3. SQLite transaction, migration, crash, and outbox tests;
4. GitHub/Hermes adapter fixtures and adversarial payload tests;
5. process cleanup, timeout, and model-mismatch tests;
6. offline end-to-end case simulations with restart injection;
7. disposable-systemd install, upgrade, failure, and rollback tests;
8. Python-reference/Rust decision-parity fixtures during migration;
9. non-dispatching live reconciliation shadow; and
10. one explicitly activated MDK shadow case.

CI, local tests, systemd lifecycle validation, live shadow evidence, and
production activation are distinct gates and must be reported separately.

## 19. Definition of done

Pip v2 is ready to expand beyond the MDK canary when:

- repository/issue identity is policy-driven rather than compiled in;
- the Rust ledger is the sole workflow authority;
- every accepted worker result is immutable and reconstructable;
- dynamic remediation converges or escalates within policy bounds;
- restart/replay cannot duplicate a worker, branch, PR, or merge;
- exact-head CI and both mandatory reviews are independently revalidated;
- provider/model failure blocks without substitution or retry storms;
- deployment provenance binds the binary to reviewed source;
- installer lifecycle and rollback pass in a disposable systemd environment;
- the full MDK case completes in shadow mode and agrees acceptably with JG;
- the legacy Python service is stopped and recoverably retained; and
- autonomous merge remains disabled unless separately authorized.

Current implementation status and known gaps are tracked separately in
[`implementation-status.md`](implementation-status.md). That inventory cannot
weaken this target architecture or convert local adapter coverage into live
runtime evidence.
