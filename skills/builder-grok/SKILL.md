---
name: builder-grok
description: Use when implementing an approved Pip plan with Grok.
version: 0.9.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, build, cursor, grok]
    related_skills: [workflow-contract]
---

# Builder Grok

## Overview

The Rust direct-provider runtime starts one fresh Cursor Agent invocation using the exact policy-bound model. For the current MDK workflow that model is `cursor-grok-4.6-high-fast`. Model substitution is a blocked outcome; the skill never chooses a fallback.

## Workflow

1. Read and verify the task's `immutable_evidence_bundle`. Select the one accepted planner run for the exact active plan version and its `GITHUB_PLAN_PUBLICATION` evidence, including the controller actor, comment ID, and body digest. Independently fetch that exact comment and verify the provenance marker and digest. Treat the matching run, publication evidence, and bound plan artifacts as the authorized plan.
2. Verify the task's `assigned_worktree` is a child of the policy workspace and its `assigned_branch` is the exact case-owned `pip/*` branch. Clone or reconcile the exact repository only at that path, check out only that branch, and fetch current `master`. Record the actual implementation base. The planned base is context, not a checkout lock: adapt paths and mechanics to ordinary upstream movement. Return to planning only when a concrete upstream change makes the authorized scope unsafe, contradictory, or unimplementable; report that incompatibility precisely.
3. Verify the task's requested model is exactly the model reported by the fresh runtime session. The runtime probes model availability and constructs the single invocation before this skill runs; do not start, resume, or substitute another agent session.
4. Record that Cursor does not provide independent provider-side routing attestation; do not overstate the available assurance.
5. Implement only the authorized scope. Never change MLS/CGKA, keys, trust anchors, membership/admin authorization semantics, or push-payload context without JG authorization.
6. Add regression coverage. Run repository-native formatting, lint, tests, and full-diff review. Do not bump versions. Update the existing Unreleased changelog when code changes.
7. Create a local Pip-attributed commit on `assigned_branch` and leave `assigned_worktree` clean at that exact commit. Commit author or signature metadata is not controller trust evidence; the bound result, CI, and independent reviews are. Do not push or invoke any GitHub mutation. The worker receives no GitHub credential.
8. Report the exact local commit SHA. After accepting the result, the controller publishes the branch through an exact force-with-lease transaction, verifies the remote SHA, creates or updates the draft PR, and independently evaluates every GitHub CI attempt; do not claim a remote branch, PR number, or CI disposition.
9. Return the Rust `builder` result contract from `references/worker-result-contracts.md` in the loaded `workflow-contract` skill directory (not the target repository). Record the actual implementation base under `evidence.implementation_base_sha`. `RETURN_TO_PLANNING` must include `evidence.incompatibility.reason` and nonempty concrete `evidence.incompatibility.observations`; branch movement by itself is not an incompatibility. After validating it, call
    `kanban_complete` with a concise summary and the complete object as
    `metadata`; Hermes must durably store the contract in the Kanban run
    metadata. Then return the same object as the entire final response without
    prose or a code fence. Put durable artifact paths under `evidence`. Never merge.

## Completion

A build result is ready for controller publication only when local checks pass, the assigned local branch contains the exact reported commit, the worktree is clean, and no visible model mismatch occurred. The controller publishes the branch; remote branch identity, draft-PR identity, and GitHub CI are later controller gates. Provider-side Cursor routing is requested and recorded, not cryptographically attested.
