---
name: builder-grok
description: Use when implementing an approved Pip v2 plan with Grok.
version: 0.3.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, build, cursor, grok]
    related_skills: [workflow-contract]
---

# Builder Grok

## Overview

The Hermes `cursor-fixer` profile is the v1-style task orchestrator. It delegates the implementation itself to one fresh direct Cursor Agent invocation using `cursor-grok-4.6-high`. Model substitution is a blocked outcome.

## Workflow

1. Read the Kanban task contract. Fetch the referenced planner comment from GitHub; verify its numeric author, URL, recorded SHA-256 digest, plan version, planned base SHA, and execution disposition. Treat that comment as the authorized plan artifact.
2. Clone the exact repository into the scratch workspace and fetch current `master`. Record the actual implementation base. The planned base is context, not a checkout lock: adapt paths and mechanics to ordinary upstream movement. Return to planning only when a concrete upstream change makes the authorized scope unsafe, contradictory, or unimplementable; report that incompatibility precisely.
3. Verify `cursor-grok-4.6-high` appears in `agent --list-models`.
4. Invoke Cursor once in a fresh session:
   ```sh
   agent -p --force --output-format json \
     --model cursor-grok-4.6-high \
     --workspace <worktree> \
     <complete-bound-prompt>
   ```
   Include the task contract, active planner evidence, branch name, safety boundaries, test requirements, and result contract in the prompt. Do not use `--resume`, `--continue`, or unsupported `--no-mcps` options.
5. Reject any model identifier Cursor reports that differs from the requested model. Cursor does not provide independent provider-side routing attestation; record that limitation honestly.
6. Implement only the authorized scope. Never change MLS/CGKA, keys, trust anchors, membership/admin authorization semantics, or push-payload context without JG authorization.
7. Add regression coverage. Run repository-native formatting, lint, tests, and full-diff review. Do not bump versions. Update the existing Unreleased changelog when code changes.
8. Create a signed `agent-p1p` commit on a Pip-owned `pip/*` branch. Open or update a draft PR.
9. Independently verify the PR URL, exact head, and GitHub CI with `gh`; do not merely repeat Cursor's claims. Do not complete until CI is green on that exact head.
10. Return a schema-valid `builder-result` with `implementation_base_sha` set to the actual starting head. `RETURN_TO_PLANNING` must include `evidence.incompatibility.reason` and nonempty concrete `evidence.incompatibility.observations`; branch movement by itself is not an incompatibility. After validating it, call
    `kanban_complete` with a concise summary and the complete object as
    `metadata`; Hermes must durably store the contract in the Kanban run
    metadata. Then return the same object as the entire final response without
    prose or a code fence. Include durable artifact paths. Never merge.

## Completion

A build is review-ready only when local checks pass, exact-head GitHub CI is green, the draft PR exists, and no visible model mismatch occurred. Provider-side Cursor routing is requested and recorded, not cryptographically attested.
