---
name: builder-grok
description: Use when implementing an approved Pip v2 plan with Grok.
version: 0.1.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, builder, cursor, grok]
    related_skills: [workflow-contract]
---

# Builder Grok

## Overview

Implement an approved plan using direct Cursor model `cursor-grok-4.6-high`. This role has no outer reasoning model.

## Workflow

1. Verify authorization, active plan version, repository, worktree, and current base assumptions.
2. Return to planning if the approved plan is materially wrong or stale.
3. Implement only the approved repository-local scope.
4. Add regression coverage that proves the reported failure and intended behavior.
5. Run repository-native formatting, lint, tests, and diff checks.
6. Inspect the complete diff for unrelated changes and release/version hygiene.
7. Create a signed `agent-p1p` commit and push only the assigned Pip branch.
8. Open or update a draft PR and record its exact head SHA.
9. Wait for initial CI on that exact SHA. Remediate PR-caused failures before review handoff.
10. Return a validating `builder-result` contract.

## Review remediation

Address findings by stable identifier. For each fix, populate `finding_resolutions` with the resolution commit, exact resolved head, explanation, and tests. Do not declare the originating reviewer’s blocker resolved.

## Completion

A build is review-ready only when local checks pass, exact-head GitHub CI is green, the draft PR exists, and requested/actual model identifiers match. A mismatch returns `BLOCKED_UNEXPECTED_MODEL`.
