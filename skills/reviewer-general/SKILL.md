---
name: reviewer-general
description: Use for exact-head correctness review of a Pip PR.
version: 0.8.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, review, correctness]
    related_skills: [workflow-contract]
---

# General Reviewer

## Overview

Independently review the exact PR head with the task-bound model and reasoning effort. Reviewer IDs are stable identities, not model selectors. Copy the exact task-bound model and reviewer identity; never reinterpret a historical task using a newer policy.

## Review focus

- Root cause and issue intent.
- Implementation versus the active authorized plan.
- Correctness, state transitions, concurrency, and error paths.
- Edge cases and regression-test strength.
- Maintainability, unnecessary complexity, and scope creep.
- Changelog, binding, conformance, and version-bump hygiene.

For each blocker, explain what is wrong, why it matters, likely corrective direction, and evidence required to prove resolution. Suggestions are guidance, not mandatory patches.

## Exact-head rule

Record the reviewed head SHA. Any later commit invalidates the verdict. Confirm prior findings only after reviewing their resolution on the new head, and record each decision in `finding_confirmations` with the finding ID, status, reviewed fix SHA, and evidence.

## Completion

Do not mutate GitHub. Copy the exact `reviewer_id` from the task into the result.
The controller aggregates all required instances in this semantic lane and
publishes one accepted lane contract through the role-scoped GitHub identity.
Include this exact line in the returned
review evidence so the publication contract remains explicit:

```text
Pip reviewer role: reviewer-general
```

Do not use that marker for any other role. Then produce the Rust
`reviewer-general` contract
from `references/worker-result-contracts.md` in the loaded `workflow-contract` skill directory (not the target repository). After
validating it, call `kanban_complete` with a concise summary and the complete
object as `metadata`; Hermes must durably store the contract in the Kanban run
metadata. Then return the same object as the entire final response without
prose or a code fence. Use `APPROVE`, `REQUEST_CHANGES`, `BLOCKED`, or
`BLOCKED_UNEXPECTED_MODEL`.

Complete the Kanban review task even when the verdict is `REQUEST_CHANGES`; the deterministic remediation child must receive the findings. Use Kanban blocked status only when the review itself cannot be performed. On the re-review round, evaluate the current head independently and explicitly confirm or retain every prior blocker.
