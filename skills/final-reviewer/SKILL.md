---
name: final-reviewer
description: Use for holistic final adjudication of a Pip case.
version: 0.6.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, final-review, merge]
    related_skills: [workflow-contract]
---

# Final Reviewer

## Overview

Holistically adjudicate the complete case using the exact task-bound model and reasoning effort. Never reinterpret a historical task's model binding using a newer policy. Do not merely repeat the general code review.

## Workflow

1. Verify the task's evidence bundle (inline or via `immutable_evidence_ref`, as specified by the shared contract), including its case/revision binding and root digest. It is the authoritative closed-world history for this adjudication; do not substitute parent summaries or session memory.
2. Re-read the original issue and authoritative clarifications recorded in the bundle, then independently refresh the live issue authorization.
3. Inspect every bundled plan version and identify the active authorized plan and its controller publication evidence.
4. Inspect the final diff and every bundled build/remediation round.
5. Inspect both complete bundled review histories, findings, confirmations, controller-published review evidence, and CodeRabbit findings when present.
6. Verify the bundled `GITHUB_FINAL_PREFLIGHT` observation binds ownership, mandatory approvals, resolved threads, clean mergeability, and green CI to the task's current exact head. Independently query the current PR and all check-run attempts with `filter=all`; any historical red attempt permanently blocks that PR. Permit a skipped check only when it is the exact conditional `Publish wn-agent release` job.
7. Decide whether the work solves the right root problem with sufficient evidence.
8. Return `READY`, `RETURN_TO_BUILD`, `RETURN_TO_REVIEW`, `RETURN_TO_PLANNING`, `WAIT_FOR_ISSUE_CREATOR`, `BLOCKED`, `ABANDON`, or `BLOCKED_UNEXPECTED_MODEL`.

## Merge separation

Do not invoke merge, notify a human, or claim merge or notification authority. MDK is human-merge-only. A clean result means only that the deterministic post-validation consumer may later send JG the PR link, exact head, CI/review evidence, and a recommendation. Any unresolved blocker, reviewer mismatch, later commit, red CI, sensitive-scope change, or missing human review remains held.

## Completion

Post a final role-stamped rationale inside the Rust `final-reviewer` contract
from `references/worker-result-contracts.md` in the loaded `workflow-contract` skill directory (not the target repository), tied to the exact reviewed head. After
validating it, call `kanban_complete` with a
concise summary and the complete object as `metadata`; Hermes must durably
store the contract in the Kanban run metadata. Then return the same JSON object
as the entire final response without prose or a code fence. Use `READY` when
every gate passes; deterministic shadow policy maps it to a human-held
`SHADOW_READY` disposition. Do not send, subscribe, stage,
or otherwise trigger a human notification. Never merge.
