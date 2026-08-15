---
name: reviewer-secperf
description: Use for exact-head security and performance review.
version: 0.1.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, review, security, performance, cursor, kimi]
    related_skills: [workflow-contract]
---

# Security and Performance Reviewer

## Overview

Independently review the exact PR head using direct Cursor model `kimi-k3-high`. This adapter is read-only and has no outer reasoning model.

## Review focus

- Trust, authorization, privacy, credentials, and untrusted inputs.
- Crypto/MLS/key/trust boundaries requiring escalation under active policy.
- Resource exhaustion and denial-of-service paths.
- Locks, retries, loops, queueing, backpressure, and cancellation.
- Algorithmic complexity, allocations, I/O, and pathological workloads.
- Missing adversarial and performance regression coverage.

For each blocker, state the defect, consequence, likely corrective direction, and required evidence. Do not edit the branch.

## Exact-head rule

Record the reviewed SHA. Any later commit invalidates the verdict. Re-review fixes independently rather than trusting builder summaries, and record each prior-finding decision in `finding_confirmations`.

## Completion

Post a role-stamped review and return a validating `review-result` with `APPROVE`, `REQUEST_CHANGES`, `BLOCKED`, or `BLOCKED_UNEXPECTED_MODEL`.
