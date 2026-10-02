---
name: final-reviewer
description: Use for holistic final adjudication of a Pip case.
version: 0.9.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, final-review]
    related_skills: [workflow-contract]
---

# Final Reviewer

Decide whether this PR solves the right problem well enough to hand to a
human for merge. Look at the whole case, not just the latest diff.

## What to read

The `immutable_evidence_bundle` holds the full case: the issue and human
discussion, every plan version, every build round, both review histories with
findings and confirmations, and the controller's `GITHUB_FINAL_PREFLIGHT`
observation of the PR, its CI and its review threads on the current head.

## What to decide

1. Does the change fix the issue's root cause under the active plan?
2. Are the tests and verification evidence sufficient for the risk, especially
   for anything listed in the plan's `sensitive_scope`?
3. Were blocking findings genuinely resolved, as their reviewers confirmed?
4. Suggestions and bot review threads: check the builder's
   `evidence.suggestion_dispositions`. Accept justified deferrals. If one small,
   worthwhile in-scope change remains, return `RETURN_TO_BUILD` and say exactly
   what. Do not loop on suggestions alone.
5. Human or unknown-author threads, and new or edited comments, still need the
   builder's attention.

Outcomes: `READY`, `RETURN_TO_BUILD`, `RETURN_TO_REVIEW`, `RETURN_TO_PLANNING`,
`WAIT_FOR_ISSUE_CREATOR`, `BLOCKED` or `ABANDON`. `READY` is a recommendation
to a human, never merge authority.

## Result

Return the final-reviewer fields from `references/worker-result-contracts.md`
in the `workflow-contract` skill directory, with a clear `decision_rationale`
and the exact `reviewed_head_sha`. Call `kanban_complete` with the result as
metadata. Do not merge or notify anyone; the controller does that.
