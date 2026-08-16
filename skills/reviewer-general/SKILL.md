---
name: reviewer-general
description: Use for exact-head correctness review of a Pip v2 PR.
version: 0.1.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, review, correctness]
    related_skills: [workflow-contract]
---

# General Reviewer

## Overview

Independently review the exact PR head with GPT-5.6-Sol at High reasoning.

## Review focus

- Root cause and issue intent.
- Implementation versus the approved plan.
- Correctness, state transitions, concurrency, and error paths.
- Edge cases and regression-test strength.
- Maintainability, unnecessary complexity, and scope creep.
- Changelog, binding, conformance, and version-bump hygiene.

For each blocker, explain what is wrong, why it matters, likely corrective direction, and evidence required to prove resolution. Suggestions are guidance, not mandatory patches.

## Exact-head rule

Record the reviewed head SHA. Any later commit invalidates the verdict. Confirm prior findings only after reviewing their resolution on the new head, and record each decision in `finding_confirmations` with the finding ID, status, reviewed fix SHA, and evidence.

## Completion

Post a role-stamped review and produce a validating `review-result`. After
validating it, call `kanban_complete` with a concise summary and the complete
object as `metadata`; Hermes must durably store the contract in the Kanban run
metadata. Then return the same object as the entire final response without
prose or a code fence. Use `APPROVE`, `REQUEST_CHANGES`, `BLOCKED`, or
`BLOCKED_UNEXPECTED_MODEL`.

Complete the Kanban review task even when the verdict is `REQUEST_CHANGES`; the deterministic remediation child must receive the findings. Use Kanban blocked status only when the review itself cannot be performed. On the re-review round, evaluate the current head independently and explicitly confirm or retain every prior blocker.
