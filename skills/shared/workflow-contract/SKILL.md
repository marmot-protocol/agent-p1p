---
name: workflow-contract
description: Use for every Pip v2 case task. Enforce shared invariants.
version: 0.1.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, workflow, contracts, exact-head]
    related_skills: []
---

# Pip v2 Workflow Contract

## Overview

This is the shared contract for every Pip v2 role. Role-specific skills add responsibilities but may not weaken these invariants.

## Invariants

1. Start from durable case artifacts and current source state. Do not rely on prior session memory.
2. Work only on the assigned repository, issue, case, and Pip-owned branch.
3. Never expose credentials or secrets in output, logs, comments, or artifacts.
4. Record requested and actual models. If they differ, return `BLOCKED_UNEXPECTED_MODEL`.
5. Copy `route_id`, `comment_id`, `evidence_body_sha256`, `planner_comment_id`, `planner_body_sha256`, and `planned_base_sha` exactly from the task's authorization binding into every non-planner result.
6. Bind CI and review evidence to an exact 40-character PR head SHA.
7. Do not treat CodeRabbit as mandatory; concrete findings are still actionable. If a CodeRabbit status exists but says the review was rate limited, do not represent it as complete evidence.
8. A PR that had any red CI attempt is permanently ineligible. A green rerun does not clear that history.
9. Do not silently broaden scope or edit a dependency repository.
10. Human takeover or removed authorization stops the case.
11. Complete the versioned structured result contract before reporting success.
12. Never merge directly from a planning, building, or review role.
13. Parent summaries may be truncated. Resolve every parent with `hermes kanban --board pip-mdk show <task-id> --json`, read the full run metadata, and dereference the declared result artifact before relying on PR numbers, findings, or remediation evidence.

## Ownership

Only Pip-authored `pip/*` work is eligible. Existing human-owned PRs and human-held cases fail closed. Technical access is not authorization.

## Completion

A run is complete only when its durable artifacts exist, its JSON result validates, and all claimed evidence can be fetched independently.

## Common pitfalls

- Reusing an earlier CI result after the head changed.
- Calling a finding resolved before the originating reviewer confirms it.
- Guessing product intent from code.
- Treating an external reviewer outage as approval.
- Returning prose without the required structured result.

## Verification checklist

- [ ] Scope and authorization are current.
- [ ] Requested and actual models match.
- [ ] Every SHA-specific claim references the current SHA.
- [ ] Output validates against the role schema.
- [ ] No secrets or unrelated repository changes appear.
