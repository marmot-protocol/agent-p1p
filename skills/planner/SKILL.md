---
name: planner
description: Use when validating and planning a pip-ok issue.
version: 0.2.0
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
5. Identify product, protocol, design, privacy, trust, persistence, or API questions code cannot answer.
6. Return to an authoritative human with focused questions when intent is ambiguous.
7. Record technically unambiguous cross-repository prerequisites without editing that repository.
8. Define scope, non-scope, implementation sequence, regression tests, verification commands, risks, and invariants.
9. Publish a readable issue comment plus versioned Markdown and JSON plan artifacts.
10. Return a `planner-result` contract.

## Stop outcomes

Use `WAITING_FOR_ISSUE_CREATOR`, `NEEDS_HUMAN_SCOPE_DECISION`, `CROSS_REPO_DEPENDENCY`, `ALREADY_FIXED`, `NOT_REPRODUCIBLE`, `DUPLICATE`, `ABANDON`, or `BLOCKED_UNEXPECTED_MODEL` instead of inventing missing intent or accepting model substitution.

## Completion

Planning is complete only when the issue comment and both plan artifacts agree, contain the planned base SHA, and the structured result validates.
