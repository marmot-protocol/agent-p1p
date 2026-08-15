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
7. Return `MERGE`, `RETURN_TO_BUILD`, `RETURN_TO_REVIEW`, `RETURN_TO_PLANNING`, `WAIT_FOR_ISSUE_CREATOR`, `BLOCKED`, `ABANDON`, or `BLOCKED_UNEXPECTED_MODEL`.

## Merge separation

Do not directly invoke merge. A `MERGE` result authorizes only the deterministic guarded merge transaction, which must re-fetch and revalidate live state.

## Completion

Post a final role-stamped rationale and return a validating `final-result` tied to the exact reviewed head.
