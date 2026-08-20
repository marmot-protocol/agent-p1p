---
name: reviewer-secperf
description: Use when reviewing a Pip v2 PR for security and performance.
version: 0.2.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, review, security, performance, cursor, kimi]
    related_skills: [workflow-contract]
---

# Reviewer — Security and Performance

## Overview

The Hermes `cursor-reviewer` profile is the v1-style task orchestrator. It delegates the substantive read-only review to one fresh direct Cursor Agent invocation using `claude-opus-4-8-thinking-high`. It must be independent of the builder and general reviewer.

## Workflow

1. Resolve the parent draft PR through GitHub. Record its exact head SHA and verify CI is green on that head before review.
2. Verify `claude-opus-4-8-thinking-high` appears in `agent --list-models`.
3. Clone or fetch the repository in the scratch workspace and check out the exact PR head without modifying or pushing it.
4. Invoke Cursor once in a fresh read-only session:
   ```sh
   agent -p --mode plan --output-format json \
     --model claude-opus-4-8-thinking-high \
     --workspace <checkout> \
     <complete-review-prompt>
   ```
   Include the issue, active planner evidence, exact PR/head, full diff, security/performance rubric, and review-result contract. Do not use `--resume`, `--continue`, or unsupported `--no-mcps` options.
5. Reject any model identifier Cursor reports that differs from the request. Record that Cursor does not independently attest provider-side routing.
6. Review trust boundaries, data exposure, unsafe parsing, misuse/abuse paths, resource bounds, algorithmic regressions, concurrency, and denial-of-service risk. Treat any unexpected MLS/CGKA, key, trust-anchor, authorization-semantic, or push-context change as blocking and escalate to JG.
7. Independently verify every material claim against the exact checkout and GitHub. Do not alter branches, commits, PR text, labels, or code.
8. Return a schema-valid `review-result`. After validating it, call
   `kanban_complete` with a concise summary and the complete object as
   `metadata`; Hermes must durably store the contract in the Kanban run
   metadata. Then return the same object as the entire final response without
   prose or a code fence. Include exact head, findings, confidence,
   requested/reported model evidence, and durable artifact paths.

## Blocking rule

Any security regression, unresolved high-impact performance issue, unauthorized sensitive change, visible model mismatch, stale head, or red CI blocks progression. Fixes require a fresh same-head re-review.

Return blocking findings in the result and complete the Kanban review task so the deterministic remediation child can run. Use Kanban blocked status only when the review itself cannot be performed. On the second review round, explicitly confirm or retain every prior blocker on the current exact head.
