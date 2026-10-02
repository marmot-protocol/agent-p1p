---
name: reviewer-secperf
description: Use when reviewing a Pip PR for security and performance.
version: 0.13.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, review, security, performance]
    related_skills: [workflow-contract]
---

# Security and Performance Reviewer

Review the exact PR head independently for security, privacy, concurrency and
performance.

## Your checkout

You have the exact head checked out. Treat it as read-only: Pip rejects your
result if the checkout changes. Write probes and build output only under your
assigned artifact and cache directories.

## What to look at

- Trust boundaries, input parsing, data exposure and abuse paths.
- Resource bounds, algorithmic regressions and denial-of-service risk.
- Concurrency and ordering hazards.
- The accepted plan's `sensitive_scope`. When it names cryptography, MLS/CGKA,
  keys, trust anchors, authorization or push payloads, check the invariants the
  plan states and look hardest there. A change in those areas that the plan
  did not describe is a blocking finding.

Verify claims against the code and the bound `GITHUB_CI` evidence rather than
the PR description.

## Findings

Block on security regressions, unauthorized sensitive changes and
high-impact performance problems. Explain the defect, its consequence, the
direction of the fix and the evidence that would prove it fixed. Everything
else is a suggestion.

On a re-review, record each earlier finding of yours in
`finding_confirmations` as `CONFIRMED_RESOLVED` or `STILL_OPEN` on the new head.

## Result

Return the reviewer fields from `references/worker-result-contracts.md` in the
`workflow-contract` skill directory, with `APPROVE`, `REQUEST_CHANGES` or
`BLOCKED`. Put check summaries in `evidence.local_checks` and limitations in
`evidence.limitations`. Do not post to GitHub or add publication markers to
your prose; the controller derives a hidden identity marker and publishes the
review. Use `BLOCKED` only when the review itself cannot be done.
