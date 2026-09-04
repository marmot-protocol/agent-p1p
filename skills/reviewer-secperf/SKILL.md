---
name: reviewer-secperf
description: Use when reviewing a Pip PR for security and performance.
version: 0.6.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, review, security, performance, cursor, kimi]
    related_skills: [workflow-contract]
---

# Reviewer — Security and Performance

## Overview

The Rust direct-provider runtime starts one fresh read-only Cursor Agent invocation using the exact policy-bound model. The current MDK workflow runs required `secperf-kimi` with `kimi-k3-max` and shadow `secperf-opus` with `claude-opus-5-thinking-high`. Each instance is independent of the builder, the general lane, and the other security/performance instance. The skill never chooses a model, mode, or fallback.

## Workflow

1. Resolve the parent draft PR through GitHub. Record its exact head SHA and verify CI is green on that head before review.
2. Verify the task's requested model and `reviewer_id` exactly match the fresh runtime session and immutable binding. The runtime probes model availability and constructs the single invocation before this skill runs; do not start, resume, or substitute another agent session. Record that Cursor does not independently attest provider-side routing.
3. Use the assigned exact-head checkout read-only. Do not modify or push it.
4. Review trust boundaries, data exposure, unsafe parsing, misuse/abuse paths, resource bounds, algorithmic regressions, concurrency, and denial-of-service risk. Treat any unexpected MLS/CGKA, key, trust-anchor, authorization-semantic, or push-context change as blocking and escalate to JG.
5. Independently verify every material claim against the exact checkout and GitHub evidence. Do not alter branches, commits, PR text, labels, or code.
6. Do not mutate GitHub. Copy the exact `reviewer_id` into the result. Required instances participate in the lane verdict; advisory and shadow instances are recorded as immutable observations and never advance or block the workflow. The controller publishes only the aggregate required lane contract through
   the role-scoped reviewer identity. Include this exact line in the returned
   review evidence so the publication contract remains explicit:
   ```text
   Pip reviewer role: reviewer-secperf
   ```
   Do not use that marker for any other role. Return the Rust
   `reviewer-secperf` contract from `docs/worker-result-contracts.md`. Put confidence, provider limitations, and durable artifact paths under `evidence`. After validating it, call
   `kanban_complete` with a concise summary and the complete object as
   `metadata`; Hermes must durably store the contract in the Kanban run
   metadata. Then return the same object as the entire final response without
   prose or a code fence.

## Blocking rule

Any security regression, unresolved high-impact performance issue, unauthorized sensitive change, visible model mismatch, stale head, or red CI blocks progression. Fixes require a fresh same-head re-review.

Return blocking findings in the result and complete the Kanban review task so the deterministic remediation child can run. Use Kanban blocked status only when the review itself cannot be performed. On the second review round, explicitly confirm or retain every prior blocker on the current exact head.
