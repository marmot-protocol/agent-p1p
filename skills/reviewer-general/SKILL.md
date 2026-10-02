---
name: reviewer-general
description: Use for exact-head correctness review of a Pip PR.
version: 0.11.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, review, correctness]
    related_skills: [workflow-contract]
---

# General Reviewer

Review the exact PR head independently, as an experienced engineer on this
codebase would.

## What to look at

- Does the change fix the issue's root cause, as the accepted plan describes?
- Correctness: state transitions, concurrency, error paths, edge cases.
- Are the tests strong enough to catch a regression of this bug?
- Maintainability: unnecessary complexity, scope creep, missing changelog.

Read the code and run the relevant tests in your checkout. Bind what you say
to the head you reviewed and report it as `reviewed_head_sha`.

## Findings

A blocking finding is something that must change before a human should merge:
explain the defect, why it matters, the direction of the fix, and the evidence
that would prove it fixed. Everything else is a suggestion. Do not block on
style or preference.

On a re-review, check each of your earlier findings against the new head and
record it in `finding_confirmations` as `CONFIRMED_RESOLVED` or `STILL_OPEN`.
Do not repeat a finding that has been fixed, and do not restate a still-open
one in new words; confirm it as `STILL_OPEN`.

## Result

Return the reviewer fields from `references/worker-result-contracts.md` in the
`workflow-contract` skill directory, with `APPROVE`, `REQUEST_CHANGES` or
`BLOCKED`. Put check summaries in `evidence.local_checks` and limitations in
`evidence.limitations`. Do not post to GitHub or add publication markers to
your prose; the controller derives a hidden identity marker and publishes the
review. Call `kanban_complete` with the result as metadata, including when you
request changes. Use `BLOCKED` only when the review itself cannot be done.
