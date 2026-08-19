---
name: planner
description: Use when validating and planning a pip-ok issue.
version: 0.3.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, planning, root-cause]
    related_skills: [workflow-contract]
---

# Planner

## Overview

Validate an authorized issue, identify its actual root cause, and produce a versioned implementation plan before code is written. Run with GPT-5.6-Sol at `xhigh` reasoning.

## Workflow

1. In the assigned scratch workspace, create a read-only clone of the named repository (or use an explicitly supplied immutable checkout). Fetch the live issue, comments, labels, current `master`, and related work. Never push from planning.
2. Establish whether the behavior remains real, unfixed, and correctly described.
3. Distinguish the root cause from its symptoms.
4. Determine whether the fix is repository-local.
5. Identify product, protocol, design, privacy, trust, persistence, or API questions code cannot answer. Risk alone is not ambiguity: define bounded invariants and tests when intent is clear.
6. Default to `PROCEED` for technically unambiguous, repository-local work. Return to an authoritative human only for a concrete unresolved product/scope decision, cross-repository dependency, or changes involving MLS/CGKA, keys, trust anchors, membership/admin authorization semantics, or push-payload context.
7. Record technically unambiguous cross-repository prerequisites without editing that repository.
8. Define scope, non-scope, implementation sequence, regression tests, verification commands, risks, and invariants.
9. Publish a new immutable issue comment for each plan version plus versioned Markdown and JSON plan artifacts; never edit an earlier planner comment in place. The exact outcome in the heading is a machine-consumed execution disposition: `PROCEED` authorizes ordinary builder dispatch; human-wait outcomes do not. The planned base SHA is an analysis snapshot, not a checkout lock. Every result must include a one-line `authorized_scope` and a `sensitive_scope` array using only the schema categories. `PROCEED` requires no open decisions, dependencies, or sensitive scope and exactly one canonical compact sorted-JSON line: `Pip execution binding: {"authorized_scope":"<exact one-line implementation scope>","dependencies":[],"open_decisions":[],"outcome":"PROCEED","plan_version":<version>,"sensitive_scope":[],"task_id":"<current planner task id>"}`. Never use `PROCEED` when the authorized scope includes cryptography, MLS/CGKA, key handling, trust anchors, membership/admin authorization semantics, or push-payload context. Every human-wait outcome must name a concrete open decision and must not carry an execution binding. When replanning after `Pip: narrow scope — …`, preserve that scope exactly as the execution binding's `authorized_scope` and add one single-line binding to the planner comment: `Pip narrowing binding: {"body_sha256":"<64 hex>","comment_id":<id>,"narrowed_scope":"<exact scope>"}`. Use canonical compact JSON with sorted keys; never broaden or paraphrase the narrowed scope.
   Only for a human-wait outcome, tell the authoritative human that approval may be a complete comment containing `approve`, `approved`, `@agent-p1p approve`, or `@agent-p1p approved`; rejection accepts the corresponding `reject`/`rejected` forms. Extra prose is not accepted. Narrowing still requires `Pip: narrow scope — <one-line scope>`.
10. Return a `planner-result` contract.

## Stop outcomes

Use `WAITING_FOR_ISSUE_CREATOR`, `NEEDS_HUMAN_SCOPE_DECISION`, `CROSS_REPO_DEPENDENCY`, `ALREADY_FIXED`, `NOT_REPRODUCIBLE`, `DUPLICATE`, `ABANDON`, or `BLOCKED_UNEXPECTED_MODEL` instead of inventing missing intent or accepting model substitution.

Do not use a human-wait outcome merely because `master` moved, implementation has ordinary merge risk, or the work requires careful tests. Use it only when code and repository policy cannot resolve the decision.

## Completion

Planning is complete only when the issue comment and both plan artifacts agree, contain the planned base SHA, and the structured result validates.
