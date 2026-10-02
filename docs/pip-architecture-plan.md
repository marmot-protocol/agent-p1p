# Pip architecture

This is the canonical architecture referenced by `AGENTS.md`. It describes how
Pip is meant to work; [status](status.md) says what is deployed and proven.

## Purpose

Turn an explicitly authorized GitHub issue into a draft PR with a validated
plan, an implementation, independent reviews and a final readiness
recommendation, which a human then reviews and merges. Pip is a small Rust
workflow coordinator around unmodified Hermes and a narrow Cursor adapter.

Pip's job is to keep making progress safely and to tell a human clearly when it
cannot. No model, worker or configuration merges; the MDK pilot stays in shadow
merge mode until JG changes it.

## Ownership

| Component | Authority |
|---|---|
| GitHub | Issues, labels and actors, branches, PR heads, reviews, CI, merge status |
| Rust ledger | Accepted decisions, job definitions and results, findings, intended GitHub effects |
| Hermes | Native task execution and the operator-facing board |
| Cursor adapter | Executing one direct job and returning its result |
| Human | Authorization, scope decisions, resuming paused cases, takeover, merge |

One SQLite ledger is authoritative. Hermes completion and Cursor output are
observations that the controller validates before anything advances.

## Workflow

1. A trusted actor applies the authorization label. Signed webhooks (with
   bounded polling for missed events) revalidate the issue, label actor,
   assignees, exclusions and capacity, then create a case and a planning job.
2. The planner validates the issue against current source, finds the root
   cause and returns a plan. Sensitive areas (cryptography, MLS, keys, trust
   anchors, authorization, push payloads) are listed in the plan and focus the
   security review; only an open product decision holds for a human.
3. The builder implements the plan in its assigned worktree, tests it and
   commits locally. The controller signs the accepted tree, publishes the
   branch and creates or updates one draft PR.
4. CI is observed for the exact PR head. Every observed check must finish;
   the latest attempt of each check decides, and required contexts must be
   green. Failures go back to the builder with log excerpts; a real merge
   conflict does too.
5. Every required reviewer instance reviews the same head independently.
   Advisory and shadow reviewers are observations only.
6. Blocking findings return to the builder. A new head needs fresh CI and
   reviews; approvals never carry over to a different commit.
7. A fresh final review looks at the whole case. After a live preflight of
   authorization, exact-head CI and approvals, and mergeability, the PR is
   marked ready and Pip posts a human-held readiness recommendation.
8. A human reviews and merges. Pip records the merge.

A person who marks Pip's PR ready early or pushes their own commit takes the
case over; Pip explains that and stops.

## Jobs and results

Every job freezes its role, reviewer identity, exact provider and model,
skills version, inputs and runtime limit before dispatch. Recovery reuses the
saved definition. Restrictive controls (pause, revocation, takeover) apply to
old and new work alike.

Workers return only their judgement and work. The controller fills in the
bookkeeping it already knows (case, task, model, plan, round, timing) before
validating a result, and ignores unknown fields. Each result is bound to its
job and accepted at most once; late results cannot advance a superseded stage.

Model substitution is prevented where it can be observed: the pinned provider
flag, the provider's model probe and the Hermes profile binding. A model's
description of itself is not evidence.

The ledger records a decision and its intended effects in one transaction.
External effects reconcile through stable ownership markers and exact heads.

## Capacity

Issue admission and worker capacity are separate limits. Optional policy
`execution_capacity` bounds native sessions, builders, direct reviewers, plan
lookahead and per-task Cargo jobs. Cases that are ready for human review or
paused waiting for a human do not hold an issue slot.

## Failure handling

Every failure is classified before Pip decides what to do:

| Kind | Examples | What happens |
|---|---|---|
| Transient | Provider throttling, quota or outage, expired login, network, lease expiry, executor crash | Retry with exponential backoff. Never spends a work budget. If one stage cannot run for `max_outage_seconds` (6h), park. |
| Work failure | Invalid or unusable result, failing build, crashed Hermes task | Retry the same job after a short cooldown, with the previous error passed to the worker. Each stage has its own budget (`max_provider_failures`); a new stage, round or head starts fresh. When spent, park. |
| Substantive | CI failures, blocking review findings | Remediation rounds (`max_remediation_rounds`). When spent, park. |
| Needs a human | Planner's open decision, worker reports it is blocked, model unavailable, policy changed, the same finding on three heads | Park immediately with the reason. |
| Stuck | No events, attempts, tasks or evidence for the stall window (longest role runtime plus two hours, at least four hours) | Park with what it was waiting on. |

A case is also bounded by an overall age (`max_case_elapsed_seconds`, 7 days).

Retrying a single Hermes job projects a fresh task for the same frozen job;
it adds no workflow transition, so peer jobs (such as the other reviewer) are
unaffected.

### Parking and resuming

Parked states are `ESCALATED`, `BLOCKED` and `WAITING_HUMAN`. Parking posts a
specific comment on the issue: what stopped, where, the last error, and how to
continue. A parked case frees its issue slot.

A trusted actor replies on the issue or PR:

- `@<pip-login> resume`: continue where it stopped. If the remediation rounds
  were used up, three more are granted.
- `@<pip-login> replan`: start again from planning.
- `@<pip-login> abandon`: stop. Re-adding the authorization label restarts it.

Lines after the command are passed to the next worker as human guidance.
Commands arrive by webhook, with a periodic poll of paused cases as a
fallback. The controller applies a command online once no builder for the case
is still running and a slot is free. Resuming rebinds the case to the current
policy, starts a fresh age window and acknowledges where work continues. None
of this requires root, stopped services or ledger edits.

### Policy changes

Operational settings (capacity, retry and time limits, the conversation inbox)
apply to existing cases immediately. Settings that define what a case accepted
(models, reviewers, actors, label, paths, merge mode, remediation rounds) stay
pinned; changing them parks existing cases with `POLICY_CHANGED`, and resuming
adopts the new settings.

## GitHub conversations

Opt-in mentions and human feedback use the same webhook, queue and publication
boundaries, with a separate inbox in the ledger. A conversation task answers a
question or recommends a replan; only the controller hands feedback to a case.
Control commands are handled separately and never wait in that inbox. See
[GitHub conversations](github-conversations.md).

## Reviews and readiness

Reviewer instances are policy records: stable ID, semantic lane (general or
security/performance), exact model, executor and mode (required, advisory,
shadow). Lane reviews publish through distinct GitHub App identities.

Readiness requires current authorization and ownership, the accepted build,
green required CI, every required review and the final review on the same
current head, no unresolved mandatory findings or human review threads, and no
merge conflict. Branch states such as "behind" or "blocked by required
approval" are left to the human who merges.

Published text is for humans: outcomes, scope, findings, checks and
limitations, with small hidden ownership markers. Structured evidence stays in
the ledger.

The controller-only signing key signs the exact accepted tree with validated
parents; workers never receive it or any GitHub credential.

## Intake and configuration

Eligibility checks the current label, a trusted numeric actor, the open issue,
assignees (anything other than Pip's account blocks admission), exclusions,
pause and capacity. Active intake assigns Pip with GitHub's additive assignee
endpoint and verifies it before creating a case. Removing the label or taking
over stops new work and publication; it does not promise to kill a running
model.

Repository, actor, model and limit configuration lives in validated policy,
never in the engine.

## Runtime and storage

Controller secrets and the ledger are separate from code-executing workers.
Workers run repository tests and provider CLIs without ledger access or GitHub
credentials. Systemd protections match each service's actual needs.

One case workspace layout, one assigned branch, controlled publication.
Worker-controlled Git hooks, credential helpers, URL rewrites and config
injection are disabled before credential-bearing operations. Review jobs get
detached exact-head copies, so a late review cannot observe the next build.

Workspaces and build output live on the managed volume with a free-space
reserve; cleanup never deletes active work or unpreserved commits.

## Packaging

One Rust executable with a pure domain core, a durable store and narrow
GitHub, Hermes and Cursor adapters. Signed release verification and an atomic
install/rollback boundary. Hermes stays upstream and is checked by a small
compatibility suite.

## Verification

Test-driven development at real boundaries: fixtures captured from actual
Hermes, Cursor and GitHub output. The proof ladder is unit and adapter tests,
transaction and restart tests, service-identity execution, verified release
installation, and real issues through to human-held readiness. Local tests,
installed behavior and live evidence are distinct gates.
