---
name: final-reviewer
description: Use for holistic final adjudication of a Pip v2 case.
version: 0.1.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, final-review, merge]
    related_skills: [workflow-contract]
---

# Final Reviewer

## Overview

Holistically adjudicate the complete case using GPT-5.6-Sol at `xhigh` reasoning. Do not merely repeat the general code review.

## Workflow

1. Re-read the original issue and authoritative clarifications.
2. Inspect all plan versions and the active approved plan.
3. Inspect the final diff and every build/remediation round.
4. Inspect both review histories, CodeRabbit findings when present, and resolution evidence.
5. Verify the join bundle binds mandatory approvals and green CI to the current exact head.
6. Decide whether the work solves the right root problem with sufficient evidence.
7. Return `HUMAN_REVIEW_REQUIRED`, `RETURN_TO_BUILD`, `RETURN_TO_REVIEW`, `RETURN_TO_PLANNING`, `WAIT_FOR_ISSUE_CREATOR`, `BLOCKED`, `ABANDON`, or `BLOCKED_UNEXPECTED_MODEL`.

## Merge separation

Do not invoke merge, notify a human, or claim merge or notification authority. MDK is human-merge-only. A clean result means only that the deterministic post-validation consumer may later send JG the PR link, exact head, CI/review evidence, and a recommendation. Any unresolved blocker, reviewer mismatch, later commit, red CI, sensitive-scope change, or missing human review remains held.

## Completion

Post a final role-stamped rationale inside a validating `final-result` tied to
the exact reviewed head. After validating it, call `kanban_complete` with a
concise summary and the complete object as `metadata`; Hermes must durably
store the contract in the Kanban run metadata. Then return the same JSON object
as the entire final response without prose or a code fence. Use
`HUMAN_REVIEW_REQUIRED` when every gate passes. Do not send, subscribe, stage,
or otherwise trigger a human notification. Never merge.
