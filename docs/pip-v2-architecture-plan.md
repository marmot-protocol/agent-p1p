# Pip v2: Repository-Scoped Autonomous Fix Pipeline

**Status:** Runtime foundation implemented; MDK pilot archived and disabled pending explicit activation
**Audience:** JG, Pip, and future implementers
**Purpose:** Provide one durable reference for the agreed target architecture, unresolved policy decisions, pilot rollout, and acceptance criteria.

> This document records the design agreed in discussion. It does not authorize changes to the current Pip pipeline, repository policies, human-held PRs, or protected worktrees. Implementation begins only after JG explicitly requests it.

---

## 1. Goal

Build a durable, conservative autonomous issue-fixing system that:

1. Validates an issue and identifies its actual root cause before code is written.
2. Produces a versioned implementation plan that is readable by humans and machine-consumable.
3. Uses Cursor Grok 4.6 High as the builder.
4. Requires independent general and security/performance reviews of the same exact PR head.
5. Iterates through immutable build and review rounds until the work converges or reaches a bounded escalation condition.
6. Performs a final holistic GPT-5.6-Sol review of the complete issue-to-PR history.
7. Merges only after deterministic exact-head revalidation.
8. Isolates each watched repository on its own Kanban board while retaining a lightweight global control plane.
9. Detects provider authentication, quota, and model failures quickly without retry storms or silent model substitution.
10. Preserves a complete, auditable record of plans, builds, findings, resolutions, model assignments, and decisions.

---

## 2. Design principles

- **Plan before implementation.** Validate the issue and root cause first.
- **Human intent is authoritative.** Product, protocol, design, and ambiguous API decisions return to trusted humans.
- **Repository scope is a boundary.** A builder never opportunistically edits another repository.
- **One reasoning role per worker.** Cursor roles run directly through Cursor adapters rather than through an unnecessary outer GPT agent.
- **Fresh context per task.** No long-running planner, builder, or reviewer sessions.
- **Immutable run history.** A permanent case owns immutable plan/build/review runs.
- **Exact-head evidence.** CI and reviews are valid only for the PR SHA they evaluated.
- **Deterministic orchestration.** Scripts move state; models reason about issues and code.
- **No silent fallback.** A role runs its pinned model or blocks.
- **Conservative convergence.** Uncertainty returns to planning, review, or a human rather than being guessed through.
- **Least privilege.** Builders build, reviewers review, and only a guarded deterministic transaction merges.
- **Observable failure.** Auth, quota, model, CI, and workflow failures produce explicit states and actionable alerts.

---

## 3. Target architecture

```text
                           GLOBAL CONTROL PLANE
                 provider health / quotas / dependencies
                  concurrency / alerts / pause / reports
                                  |
             +--------------------+--------------------+
             |                    |                    |
         pip-mdk            pip-transponder       pip-goggles ...
             |                    |                    |
             +-------- repository-scoped cases -------+
                                  |
                                  v
                      ISSUE INTAKE: pip-ok
                                  |
                                  v
                  PHASE 1: PLAN AND VALIDATE
                      planner / GPT-5.6-Sol xhigh
                                  |
            +---------------------+----------------------+
            |                     |                      |
     clarification         cross-repo dependency    approved plan
            |                     |                      |
       trusted human        linked board/case             v
                                                   PHASE 2: BUILD
                                               builder-grok / Cursor
                                               Grok 4.6 High
                                                        |
                                              draft PR + green CI
                                                        |
                                    +-------------------+-------------------+
                                    |                                       |
                         reviewer-general                         reviewer-secperf
                         GPT-5.6-Sol High                         Cursor Kimi K3 High
                                    |                                       |
                                    +-------------+-------------------------+
                                                  |
                                     deterministic join barrier
                                                  |
                                  findings -------+------- exact-head approved
                                      |                           |
                                  builder loop                    v
                                                     PHASE 3: FINAL REVIEW
                                                     GPT-5.6-Sol xhigh
                                                                  |
                   +----------------+----------------+-------------+---------+
                   |                |                |                       |
             return build     return review    return planning        MERGE decision
                                                                          |
                                                        deterministic merge transaction
```

---

## 4. Boards and global control plane

### 4.1 Repository boards

Create one board per watched repository, for example:

```text
pip-mdk
pip-marmot
pip-whitenoise-android
pip-whitenoise-mac
pip-goggles
pip-transponder
pip-darkmatter-linux
```

Each repository board owns:

- issue intake and case state;
- plan artifacts;
- worktree and branch references;
- PR lifecycle;
- build, review, and final-review rounds;
- exact-head evidence;
- blockers and human clarifications;
- repository-specific backlog and retention.

Separate boards must not become separate, duplicated orchestration implementations. One configurable workflow engine should operate across all boards.

### 4.2 Lightweight global control plane

The global control plane is deterministic and token-free. It owns only cross-board concerns:

- repository and board registry;
- global OpenAI and Cursor concurrency;
- provider authentication, quota, and model health;
- cross-repository dependency graph;
- duplicate issue/PR prevention;
- trusted-actor registry;
- global emergency pause;
- human-held PR registry;
- worker watchdog and retry suppression;
- aggregate reporting and audit;
- global resource limits;
- provider recovery probes.

It does not interpret code, override model verdicts, summarize away evidence, choose fixes, or silently change models.

---

## 5. Profiles, models, and reasoning

| Profile | Execution path | Model | Reasoning |
|---|---|---|---|
| `planner` | Fresh Hermes session | OpenAI/Codex GPT-5.6-Sol | `xhigh` |
| `builder-grok` | Direct Cursor worker adapter | `cursor-grok-4.6-high` | High |
| `reviewer-general` | Fresh Hermes session | OpenAI/Codex GPT-5.6-Sol | High |
| `reviewer-secperf` | Direct Cursor worker adapter | `kimi-k3-high` | High |
| `final-reviewer` | Fresh Hermes session | OpenAI/Codex GPT-5.6-Sol | `xhigh` |

The Cursor model identifiers above were verified as available on the current Cursor Ultra account during design.

### 5.1 No nested reasoning for Cursor roles

The current Pip Cursor lane uses an outer GPT-powered Hermes profile to invoke an inner Cursor agent. Pip v2 should avoid that arrangement.

A Cursor adapter should be a deterministic process that:

1. Loads the task and canonical artifacts.
2. Builds the role prompt from versioned skills.
3. Sets the exact repository worktree.
4. Invokes Cursor CLI with the pinned model.
5. Captures JSON output and logs.
6. Verifies requested versus actual model.
7. Validates the output schema.
8. Records usage and completion or failure.

It performs no independent LLM reasoning.

### 5.2 Fresh sessions

Every run starts a new session. No profile carries a long-running issue conversation across tasks. Durable context comes from:

- current GitHub issue and comments;
- versioned plan artifacts;
- permanent case state;
- prior immutable run results;
- current PR and exact head;
- current CI and review evidence;
- repository context files and canonical skills.

---

## 6. Permanent case and immutable runs

Create one permanent case per issue. Never repeatedly reopen one mutable task to represent the whole workflow.

Here, immutable means append-only through the control-plane API. SQLite guards
detect accidental bypass but are not a security boundary against the database
file owner, who can replace the file or schema. Before activation, the state
directory must be exclusively writable by the control-plane OS identity and
must not be exposed to planner, builder, or reviewer roles.

Example:

```text
case: whitenoise-android#1234
  plan-v1
  build-r1
  general-review-r1
  secperf-review-r1
  build-r2
  general-review-r2
  secperf-review-r2
  final-review-r1
```

The case stores current pointers and state:

```yaml
case_id: whitenoise-android#1234
workflow_version: 2
phase: review
plan_version: 1
build_round: 2
review_round: 2
final_review_round: 0
pr_number: 1500
current_pr_head: abc123
blockers: []
dependencies: []
```

Completed runs are immutable. New evidence creates a new run. This preserves an auditable timeline and makes loop detection reliable.

---

## 7. Phase 1: planning and validation

### 7.1 Responsibilities

The `planner` must answer:

1. Is the reported behavior reproducible or otherwise established by evidence?
2. Is it still present on current `master`?
3. Has another change already fixed it?
4. Is the issue describing the defect or only a symptom?
5. What is the root cause?
6. Is the root cause inside this repository?
7. Does the correct fix require broader scope than the issue describes?
8. Are there product, protocol, design, or API decisions that code inspection cannot answer?
9. What regression test proves the defect and intended fix?
10. What repository-native checks must the builder run?

### 7.2 Outcomes

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
```

The planner must not silently turn a narrow issue into a broad architectural rewrite.

### 7.3 Human clarification

When code inspection cannot determine product, protocol, design, UX, or API intent, the planner posts a focused issue comment explaining:

- what it validated;
- the specific ambiguity;
- why code inspection cannot resolve it;
- concrete options or questions;
- consequences of each option;
- that implementation is paused.

The case enters `WAITING_FOR_ISSUE_CREATOR` or `NEEDS_HUMAN_SCOPE_DECISION`.

Only comments from authoritative actors can resume planning. The trusted-actor policy must include at least JG, the issue creator where appropriate, repository maintainers, and explicitly trusted agents. Arbitrary comments cannot redefine scope.

A trusted reply creates a new planning run. It never wakes the builder directly.

### 7.4 Cross-repository dependencies

A builder must not edit another repository to satisfy its issue. The planner records and links the prerequisite.

Pip may automatically create a dependency issue only when the required contract is technically unambiguous, such as exposing an existing field or operation through an established binding. Product, protocol, persistence, privacy, trust, or public-API decisions require human input first.

Cross-board dependency metadata:

```yaml
source_board: pip-whitenoise-android
source_issue: 1234
dependency_board: pip-mdk
dependency_issue: 1400
dependency_type: blocking
required_artifact: MarmotKit binding for timeline date seek
required_before: build
```

Creating a dependency issue does not unblock the source issue. The source unblocks only when the artifact is consumable.

### 7.5 Plan artifacts

A successful planning run creates three synchronized artifacts:

1. **Readable GitHub issue comment** — human audit and discussion.
2. **Structured case metadata** — orchestration and validation.
3. **Versioned plan files** — complete builder handoff.

Suggested artifact location:

```text
<board-state>/artifacts/<case-id>/plan-v1.md
<board-state>/artifacts/<case-id>/plan-v1.json
```

The local artifact is not the only durable copy. The GitHub comment must contain enough information to reconstruct the plan.

Suggested plan sections:

- validation evidence;
- root cause;
- scope and non-scope;
- dependencies;
- ordered implementation steps;
- regression coverage;
- verification commands;
- risks and invariants;
- open decisions;
- planned base SHA.

---

## 8. Phase 2: build and review convergence

### 8.1 Builder

`builder-grok` receives the issue, approved plan, plan version, current repository state, and any unresolved findings.

It must:

1. Confirm the approved plan still applies.
2. Stop and return to planning if it discovers a material plan flaw.
3. Create or reuse the case worktree safely.
4. Implement the approved plan.
5. Add the required regression coverage.
6. Run local repository checks.
7. Inspect its own diff.
8. Create a signed commit attributed to `agent-p1p`.
9. Push a Pip-owned branch.
10. Open or update a draft PR.
11. Record the exact head SHA.
12. Wait for CI on that exact head.
13. Remain in builder remediation until the initial CI pass is green.

Review fan-out begins only after initial exact-head CI is green.

### 8.2 Parallel reviewers

Both reviewers independently inspect the same exact head.

`reviewer-general` focuses on:

- root-cause correctness;
- implementation versus plan;
- edge cases and error paths;
- concurrency and state transitions;
- regression coverage;
- maintainability and scope;
- changelog and release hygiene.

`reviewer-secperf` focuses on:

- security and privacy boundaries;
- untrusted input and authorization;
- credential and secret exposure;
- resource exhaustion and denial of service;
- locking, retries, loops, and backpressure;
- algorithmic and I/O cost;
- pathological workloads;
- missing adversarial tests.

Reviewers must explain what is wrong, why it matters, a likely corrective direction, and what evidence should prove resolution. Suggestions guide the builder but are not blindly authoritative.

### 8.3 CodeRabbit

CodeRabbit is advisory and optional:

- rate limiting or unavailability does not block convergence;
- skipped/rate-limited output is recorded as unavailable, not green;
- concrete findings, when present, enter the remediation loop;
- CodeRabbit never substitutes for either mandatory internal reviewer.

### 8.4 Findings and remediation

Every finding has a stable identifier:

```text
GENERAL-R2-001
SECPERF-R2-003
CODERABBIT-abc123
FINAL-R1-002
```

Builder remediation records the resolution commit, explanation, and tests. The originating reviewer confirms resolution on the new exact head. A builder cannot clear a reviewer blocker by assertion.

Any new commit invalidates both prior approvals and creates fresh reviewer runs.

### 8.5 Deterministic join barrier

The token-free join barrier requires:

```text
current PR head = X
general reviewer APPROVE at X
security/performance reviewer APPROVE at X
required CI green at X
no unresolved mandatory finding
no unresolved blocking thread
Pip owns the PR and branch
issue authorization remains valid
GitHub reports the PR mergeable without conflict
```

It must reject stale approvals, earlier-head CI, hollow/rate-limited external checks, and unconfirmed fixes.

Possible outcomes:

```text
READY_FOR_FINAL_REVIEW
RETURN_TO_BUILDER
WAITING_FOR_GENERAL_REVIEW
WAITING_FOR_SECURITY_REVIEW
WAITING_FOR_CI
BLOCKED_BY_CONFLICT
BLOCKED_BY_AUTHORIZATION
```

---

## 9. Phase 3: final holistic review

`final-reviewer` independently reads:

- original issue and human clarifications;
- all plan versions and the active plan;
- cross-repository dependencies;
- final PR diff;
- every build round;
- both review histories;
- CodeRabbit findings when present;
- each resolution and confirming review;
- regression tests and exact-head CI;
- join-barrier evidence.

It asks whether the system solved the right problem, not merely whether the diff looks plausible.

Outcomes:

```text
MERGE
RETURN_TO_BUILD
RETURN_TO_REVIEW
RETURN_TO_PLANNING
WAIT_FOR_ISSUE_CREATOR
BLOCKED
ABANDON
```

A final-review rejection becomes a tracked finding. If code changes, CI and both specialized reviews run again before another final review. If the root-cause plan was wrong, planning produces a new version before building resumes.

### 9.1 Merge authority

The final reviewer should return a structured `MERGE` decision but should not directly invoke a free-form merge command. The deterministic orchestrator performs the guarded merge transaction:

1. Fetch current PR head.
2. Confirm it matches the final reviewed SHA.
3. Confirm both mandatory approvals match that SHA.
4. Confirm required CI remains green.
5. Confirm no new blocking review or comment appeared.
6. Confirm GitHub still reports clean mergeability.
7. Confirm issue authorization remains active.
8. Merge.
9. Fetch and confirm the PR is merged.
10. Record the merge commit SHA.

Any changed state aborts the transaction and returns the case to the appropriate phase.

---

## 10. CI and base-branch policy

### 10.1 CI history

Historical red CI does not permanently poison a PR. The requirement is:

```text
The final exact PR head has green required CI before merge.
```

Failures caused by the PR return to the builder. Infrastructure failures retry with bounded backoff. Unrelated base failures should normally become a separate prerequisite issue/PR rather than broadening the current PR.

### 10.2 Master drift

Do not continuously rebase because `master` advanced.

- If GitHub reports clean mergeability, work may continue.
- Rebase only when a conflict, branch-protection requirement, or material plan invalidation requires it.
- Never merge `master` into a PR branch.
- A rebase creates a new head and invalidates all reviews.
- Avoid repeated rebases during active review.

---

## 11. Loop termination

Track:

- planning version;
- build round;
- review round;
- final-review round;
- elapsed time;
- repeated finding fingerprints;
- provider/tool failures;
- repeated unresolved objections.

Initial proposed policy, subject to JG approval:

- repeated mechanical findings may iterate normally;
- the same blocker surviving three remediation rounds escalates;
- two final-review rejections for the same architectural reason return to planning;
- planner/builder scope disagreement returns to a human;
- reviewer disagreement on security implications returns to a human;
- external dependencies enter durable blocked state rather than retrying endlessly.

Exact thresholds remain an open policy decision.

---

## 12. Skills and versioning

Maintain one canonical, version-controlled workflow repository, proposed:

```text
/home/jeff/code/pip-workflow/
  skills/
    shared/
    planner/
    builder-grok/
    reviewer-general/
    reviewer-secperf/
    final-reviewer/
```

Profile skill directories symlink to canonical skills. Shared policies are authored once and reused by all roles.

Each run records:

```yaml
workflow_version: 2
skills_repository_commit: abc123
role_skill_version: 1.0.0
```

Retain the rendered prompt/instruction artifact with each run so historical behavior is reconstructable even after a symlink target changes.

No credentials belong in the skills repository.

---

## 13. GitHub events and trusted humans

Use signed GitHub webhooks for immediate issue and PR events. Store delivery IDs for idempotency. Add a periodic reconciler to recover missed events.

Webhook events can:

- resume planning after an authoritative clarification;
- add reviewer or CodeRabbit findings;
- detect human takeover;
- detect issue closure or authorization removal;
- detect new commits or review state changes.

The reconciler is insurance, not the primary path.

Trusted actors must be centrally configured and tested. Unknown actors fail closed.

---

## 14. Provider, model, and quota monitoring

### 14.1 Required provider paths

```text
OpenAI/Codex → GPT-5.6-Sol
Cursor → cursor-grok-4.6-high
Cursor → kimi-k3-high
```

No silent fallback is permitted. Every run records requested and actual model. A mismatch blocks the result.

### 14.2 Global provider states

```text
HEALTHY
DEGRADED
LIKELY_NEAR_LIMIT
EXHAUSTED
AUTH_REQUIRED
MODEL_UNAVAILABLE
COOLDOWN
UNKNOWN
```

Track each provider/model path independently.

### 14.3 Monitoring layers

1. **Pre-dispatch gate:** auth, model, usable credential, cooldown, and concurrency.
2. **Token-free check every 5–10 minutes:** Cursor status/model availability, Codex credential state, and recent provider errors.
3. **Conditional or hourly end-to-end probe:** one minimal real inference per provider path when recent production traffic has not already proven health.
4. **Immediate failure classifier:** 401/auth, 402/credits, 429/quota, unavailable model, or unexpected model substitution.

Provider failure pauses only affected roles, preserves queued tasks, suppresses retries, and immediately alerts JG. Recovery requires one controlled successful inference probe and emits one recovery notification.

### 14.4 Usage records

Record when available:

```yaml
provider: cursor
requested_model: cursor-grok-4.6-high
actual_model: cursor-grok-4.6-high
started_at: ...
completed_at: ...
duration_seconds: ...
input_tokens: ...
output_tokens: ...
cached_tokens: ...
reported_cost: ...
case_id: ...
task_id: ...
repository: ...
role: builder-grok
```

Aggregate by repository, role, model, day, review round, and case. Estimated near-limit warnings must be labeled estimates unless the provider supplies an exact balance.

### 14.5 Credentials

Prefer one canonical shared credential pool where Hermes safely supports it. Otherwise verify all profile-local pools are synchronized and usable. Report profile-specific drift as `PROFILE_AUTH_MISMATCH`, not global provider exhaustion.

---

## 15. Canonical output contracts

All role outputs use strict versioned schemas. Missing or malformed required fields block transition; the orchestrator never guesses intent.

Minimum common fields:

```yaml
schema_version: 1
workflow_version: 2
case_id: ...
task_id: ...
role: ...
outcome: ...
requested_model: ...
actual_model: ...
skills_repository_commit: ...
started_at: ...
completed_at: ...
evidence: ...
```

Role-specific fields include plan version, base SHA, PR number, exact head SHA, findings, resolutions, test evidence, CI evidence, review URL, and final decision.

The implementation must publish JSON Schemas and validate every worker result before state transition.

---

## 16. GitHub identities and permissions

Initial implementation may use `agent-p1p` with role-stamped exact-head attestations:

```text
Pip General Review — APPROVE — SHA abc123
Pip Security/Performance Review — APPROVE — SHA abc123
```

Longer-term structural separation may use distinct GitHub Apps/tokens so:

- planner can read and comment on issues;
- builder can push Pip branches and manage draft PRs but cannot merge;
- reviewers can read and comment but cannot push or merge;
- the deterministic merge transaction alone can merge.

Separate GitHub identities are not required for the first pilot but remain a recommended hardening step.

---

## 17. Pilot coexistence with current Pip

Do not replace the existing pipeline initially.

1. Leave the existing `pip` board and profiles in place.
2. Build the five new profiles/worker definitions.
3. Build the deterministic global controller and repository workflow engine.
4. Create one pilot repository board.
5. Route only new eligible issues for that repository to Pip v2.
6. Leave existing issues and PRs on the old board until they finish or are explicitly migrated.
7. Ensure only one intake path owns each issue.
8. Route comments according to PR workflow provenance.
9. Keep all other repositories on the old pipeline.

Recommended provenance metadata:

```text
Pip-Workflow: v2
Pip-Board: <board>
Pip-Case: <case-id>
Plan-ID: <plan-id>
```

### 17.1 Shadow merge mode

The initial pilot should execute the entire workflow but stop after a final `MERGE` recommendation. JG approves the actual merge.

Evaluate:

- planner root-cause quality;
- agreement between JG and final reviewer;
- missed findings;
- unnecessary review loops;
- false blockers;
- CI and webhook reliability;
- provider usage and latency;
- state-machine recovery after interruption.

Enable autonomous guarded merge only after pilot evidence justifies it.

### 17.2 Pilot repository

The selected pilot repository is `marmot-protocol/mdk`, using board `pip-mdk`.

MDK was selected because it has a large actionable issue backlog and is highly verifiable by agents without a mobile-device or platform-specific UI test loop. The pilot begins in shadow merge mode. Existing MDK tasks and PRs remain on the legacy board, and the current MDK human-merge-only policy remains active until JG explicitly changes it after evaluating the pilot.

---

## 18. Implementation workstreams

### Workstream A: Design freeze and schemas

- Finalize state names and transitions.
- Define case and immutable-run database schema.
- Define JSON Schemas for every role.
- Define finding and resolution records.
- Define trusted-human and takeover events.
- Define loop limits.
- Define exact-head join evidence.
- Write state-machine tests before worker integration.

### Workstream B: Canonical workflow repository

- Create `/home/jeff/code/pip-workflow`.
- Add canonical shared and role-specific skills.
- Add schema files, prompt templates, adapters, and tests.
- Add version metadata and rendered-prompt retention.
- Configure symlinks from role profiles.

### Workstream C: Role profiles and adapters

- Create `planner` at GPT-5.6-Sol `xhigh`.
- Create direct Cursor `builder-grok` adapter using `cursor-grok-4.6-high`.
- Create `reviewer-general` at GPT-5.6-Sol High.
- Create direct Cursor `reviewer-secperf` adapter using `kimi-k3-high`.
- Create `final-reviewer` at GPT-5.6-Sol `xhigh`.
- Enforce fresh session per run.
- Enforce requested/actual model match.

### Workstream D: Repository workflow engine

- Implement permanent cases and immutable runs.
- Implement phase transitions.
- Implement parallel review fan-out.
- Implement deterministic join barrier.
- Implement remediation and replanning loops.
- Implement bounded escalation.
- Implement exact-head merge transaction.

### Workstream E: Global control plane

- Add board registry and global concurrency.
- Add cross-board dependencies.
- Add provider-state store and monitors.
- Add immediate alerts and recovery messages.
- Add emergency pause and human-held suppression.
- Add aggregate reports and audit.

### Workstream F: GitHub integration

- Add signed webhook processing and idempotency.
- Add periodic reconciliation.
- Add authoritative-actor handling.
- Add plan/review/final comment templates.
- Add provenance markers.
- Add exact-head and mergeability queries.

### Workstream G: Pilot

- Create the `pip-mdk` board.
- Establish the MDK intake cutover boundary.
- Run one controlled issue manually.
- Enable limited `pip-ok` intake.
- Operate in shadow merge mode.
- Compare outcomes with JG.
- Enable guarded autonomous merge only after approval.

---

## 19. Verification strategy

The workflow itself requires tests, not only live trial runs.

### State-machine tests

- successful plan-to-merge path;
- clarification and planner resumption;
- cross-board dependency blocking/unblocking;
- builder CI failure and remediation;
- one reviewer blocks while the other approves;
- new commit invalidates both approvals;
- final reviewer returns to build;
- final reviewer returns to planning;
- repeated blocker reaches escalation limit;
- human takeover freezes the case;
- provider outage pauses only affected roles;
- provider recovery resumes exactly once;
- webhook replay is idempotent;
- missed webhook is recovered by reconciliation;
- merge transaction aborts when head changes;
- clean master movement does not cause needless rebase;
- conflicting base movement returns to builder.

### Adapter tests

- exact Cursor model invocation;
- actual-model mismatch rejection;
- malformed result rejection;
- timeout and process cleanup;
- read-only enforcement for reviewer adapter;
- usage and log capture;
- no secret leakage in artifacts.

### Pilot acceptance criteria

- No duplicate PRs from old and new intake.
- Every run starts fresh and records its skill/model versions.
- Every approval is bound to an exact SHA.
- No stale approval crosses the join barrier.
- Provider failures alert quickly and do not retry storm.
- Human clarification reliably resumes planning.
- The complete case can be reconstructed after restart.
- Final reviewer and JG show acceptable agreement during shadow mode.
- The old pipeline remains operational for non-pilot repositories.

---

## 20. Safety and ownership invariants

Until explicitly changed, implementation must preserve existing ownership and hold boundaries:

- Never touch a human-owned/non-`pip/*` PR without explicit authorization.
- Human takeover freezes active and queued work and suppresses future automation.
- No task may revive a human-held PR.
- Crypto, MLS, CGKA, key handling, and trust-anchor work remains subject to the active Pip charter and escalation rules.
- MDK and other current human-merge-only policies remain in force until JG explicitly changes them during rollout.
- Technical host access does not imply authorization.
- Never expose provider credentials or tokens in logs, artifacts, comments, or alerts.

The pilot does not implicitly alter these rules.

---

## 21. Open decisions before implementation

1. Approve exact loop and escalation thresholds.
2. Finalize authoritative actors for issue clarification.
3. Decide which technically unambiguous cross-repository issues Pip may create automatically.
4. Decide when each repository may leave shadow merge mode.
5. Decide whether initial reviewer attestations share `agent-p1p` or use separate identities.
6. Confirm shared credential-pool support and design.
7. Finalize the exact state/result JSON Schemas.
8. Decide provider probe cadence and reminder intervals.
9. Decide retention and archival policy for immutable runs and rendered prompts.
10. Decide whether and when current human-merge-only repository policies change.

These decisions do not prevent building the schemas, adapters, state machine, profiles, or shadow-mode pilot. They must be resolved before unrestricted autonomous merge.

---

## 22. Recommended rollout order

```text
Stage 0 — Approve this design and resolve pre-build schema decisions
Stage 1 — Create canonical workflow repository and schemas
Stage 2 — Create role profiles and direct Cursor adapters
Stage 3 — Build and test deterministic state machine
Stage 4 — Add global provider/control plane
Stage 5 — Add GitHub webhook and reconciliation integration
Stage 6 — Create one pilot board and manually run one controlled case
Stage 7 — Enable pilot repo intake in shadow merge mode
Stage 8 — Evaluate against JG decisions and tune
Stage 9 — Enable guarded autonomous merge for the pilot if approved
Stage 10 — Expand one repository at a time
```

---

## 23. Definition of done for Pip v2 pilot

The pilot is complete when:

- one repository has an isolated board;
- the old pipeline still handles all non-pilot work;
- new pilot issues cannot enter both pipelines;
- planning validates root cause and produces all three artifacts;
- trusted-human clarification resumes planning through webhook/reconciliation;
- Grok builds directly through the Cursor adapter;
- initial exact-head CI is green before review fan-out;
- GPT and Kimi independently review the same exact head;
- findings produce immutable remediation and re-review rounds;
- CodeRabbit findings are consumed when present but outages do not block;
- the join barrier rejects stale evidence;
- final GPT review reconstructs the complete case;
- the deterministic merge transaction aborts safely on changed state;
- provider/auth failures alert JG quickly and pause only affected roles;
- case state survives process and gateway restarts;
- shadow-mode decisions are auditable and comparable with JG’s verdict;
- autonomous merge remains disabled until JG explicitly approves it.
