---
name: builder-grok
description: Use when implementing an approved Pip v2 plan with Grok.
version: 0.6.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, build, cursor, grok]
    related_skills: [workflow-contract]
---

# Builder Grok

## Overview

The Hermes `cursor-fixer` profile is the v1-style task orchestrator. It delegates the implementation itself to one fresh direct Cursor Agent invocation using `composer-2.5`. Model substitution is a blocked outcome.

## Workflow

1. Read and verify the task's `immutable_evidence_bundle`. Select the one accepted planner run for the exact active plan version and its `GITHUB_PLAN_PUBLICATION` evidence, including the controller actor, comment ID, and body digest. Independently fetch that exact comment and verify the provenance marker and digest. Treat the matching run, publication evidence, and bound plan artifacts as the authorized plan.
2. Clone the exact repository into the scratch workspace and fetch current `master`. Record the actual implementation base. The planned base is context, not a checkout lock: adapt paths and mechanics to ordinary upstream movement. Return to planning only when a concrete upstream change makes the authorized scope unsafe, contradictory, or unimplementable; report that incompatibility precisely.
3. Verify `composer-2.5` appears in `agent --list-models`.
4. Invoke Cursor once in a fresh session:
   ```sh
   agent -p --force --output-format json \
     --model composer-2.5 \
     --workspace <worktree> \
     <complete-bound-prompt>
   ```
   Include the task contract, active planner evidence, branch name, safety boundaries, test requirements, and result contract in the prompt. Do not use `--resume`, `--continue`, or unsupported `--no-mcps` options.
5. Reject any model identifier Cursor reports that differs from the requested model. Cursor does not provide independent provider-side routing attestation; record that limitation honestly.
6. Implement only the authorized scope. Never change MLS/CGKA, keys, trust anchors, membership/admin authorization semantics, or push-payload context without JG authorization.
7. Add regression coverage. Run repository-native formatting, lint, tests, and full-diff review. Do not bump versions. Update the existing Unreleased changelog when code changes.
8. Create a signed `agent-p1p` commit and push only the assigned Pip-owned `pip/*` branch through the execution adapter's scoped push boundary. Do not open, edit, review, or merge a PR.
9. Verify the local commit and pushed branch resolve to the same exact SHA. The controller creates or updates the draft PR and independently evaluates every GitHub CI attempt; do not claim a PR number or CI disposition.
10. Return the Rust `builder` result contract from `docs/worker-result-contracts.md`. Record the actual implementation base under `evidence.implementation_base_sha`. `RETURN_TO_PLANNING` must include `evidence.incompatibility.reason` and nonempty concrete `evidence.incompatibility.observations`; branch movement by itself is not an incompatibility. After validating it, call
    `kanban_complete` with a concise summary and the complete object as
    `metadata`; Hermes must durably store the contract in the Kanban run
    metadata. Then return the same object as the entire final response without
    prose or a code fence. Put durable artifact paths under `evidence`. Never merge.

## Completion

A build result is ready for controller publication only when local checks pass, the assigned branch contains the exact reported commit, and no visible model mismatch occurred. Draft-PR identity and GitHub CI are later controller gates. Provider-side Cursor routing is requested and recorded, not cryptographically attested.
