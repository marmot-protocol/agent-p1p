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

Post a role-stamped review and return a validating `review-result` with `APPROVE`, `REQUEST_CHANGES`, `BLOCKED`, or `BLOCKED_UNEXPECTED_MODEL`.
