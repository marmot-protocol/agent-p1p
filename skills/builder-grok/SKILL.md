---
name: builder-grok
description: Use when implementing an approved Pip plan with Grok.
version: 0.14.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, build, cursor, grok]
    related_skills: [workflow-contract]
---

# Builder Grok

## Overview

The Rust direct-provider runtime starts one fresh Cursor Agent invocation using the exact task-bound model. Model substitution is a blocked outcome; the skill never chooses a fallback or overrides a saved job with current policy defaults.

## Workflow

1. Read and verify the task's evidence bundle (inline or via `immutable_evidence_ref`, as specified by the shared contract). Select the one accepted planner run for the exact active plan version and its `GITHUB_PLAN_PUBLICATION` evidence, including the controller actor, comment ID, and body digest. Read the canonical implementation plan from the accepted run's `evidence.plan_markdown`. Treat that digest-bound run and the matching controller publication evidence as the authorized plan; the controller owns live GitHub revalidation. Do not seek credentials or bypass the sandbox to read a private Hermes artifact path. Historical tasks lacking an inline plan need an explicitly accessible bound artifact; otherwise block for missing plan evidence.
2. Verify the task's `assigned_worktree` is a child of the policy workspace and its `assigned_branch` is the exact case-owned `pip/*` branch. Clone or reconcile the exact repository only at that path, check out only that branch, and fetch current `master`. Record the actual implementation base. The planned base is context, not a checkout lock: adapt paths and mechanics to ordinary upstream movement. Return to planning only when a concrete upstream change makes the authorized scope unsafe, contradictory, or unimplementable; report that incompatibility precisely.
3. Verify the task's requested model is exactly the model reported by the fresh runtime session. The runtime probes model availability and constructs the single invocation before this skill runs; do not start, resume, or substitute another agent session.
4. Record that Cursor does not provide independent provider-side routing attestation; do not overstate the available assurance.
5. Implement only the authorized scope. Never change MLS/CGKA, keys, trust anchors, membership/admin authorization semantics, or push-payload context without JG authorization.
   On remediation or return-to-build, read reviewer suggestions as well as blocking findings, including the final review's rationale. Address useful, proportionate changes within the approved scope (including documentation of limitations). Do not dismiss feedback merely because it is nonblocking. Defer changes that need new scope, add disproportionate risk, duplicate completed work, or lack evidence; explain why. Do not invent blocker IDs for suggestions or edit another repository.
6. Add regression coverage. Run repository-native formatting, lint, tests, and full-diff review. Do not bump versions. Update the existing Unreleased changelog when code changes.
7. Create or reverify a local Pip-attributed commit on `assigned_branch` and leave `assigned_worktree` clean at that exact commit. A recovery may retain completed work whose earlier result was rejected: inspect and preserve that work, verify it against the current plan and findings, and reuse the commit if it already satisfies them. Do not reset retained work or create an empty/replacement commit merely because this is a new attempt. Commit author or signature metadata is not controller trust evidence; the bound result, CI, and independent reviews are. Do not push or invoke any GitHub mutation. The worker receives no GitHub credential.
8. Report the exact local commit SHA. After accepting the result, the controller publishes the branch through an exact force-with-lease transaction, verifies the remote SHA, creates or updates the draft PR, and independently evaluates every GitHub CI attempt; do not claim a remote branch, PR number, or CI disposition.
9. Return the Rust `builder` result contract from `references/worker-result-contracts.md` in the loaded `workflow-contract` skill directory (not the target repository). Record the actual implementation base under `evidence.implementation_base_sha`. `RETURN_TO_PLANNING` must include `evidence.incompatibility.reason` and nonempty concrete `evidence.incompatibility.observations`; branch movement by itself is not an incompatibility. Save and validate the object using the field guide's local validator, then return it as the entire final response without prose or a code fence. The direct runtime captures the response; do not look for Hermes completion tools or update Kanban. Put durable artifact paths under `evidence`. Never merge.

## Completion

A publication-ready result includes concise human-facing strings under `evidence`:
`pr_title` (descriptive change title, not a workflow/issue-number label),
`problem_summary` (the defect and its impact), and `solution_summary` (what was
actually changed). Keep operational identifiers and test inventories out of those
summaries. The controller adds the verified plan link and `Fixes #N` itself.

When feedback is present, include `evidence.suggestion_dispositions`: one object
per suggestion, with `reviewer_id`, the original suggestion text, `disposition`
(`addressed` or `deferred`), and a human-readable `summary` explaining the action
or reason. Include verification for addressed code changes. Retain prior
dispositions when still applicable; reassess them if the facts change. These
are evidence fields, not new top-level result fields or mandatory finding IDs.

A build result is ready for controller publication only when local checks pass, the assigned local branch contains the exact reported commit, the worktree is clean, and no visible model mismatch occurred. The controller publishes the branch; remote branch identity, draft-PR identity, and GitHub CI are later controller gates. Provider-side Cursor routing is requested and recorded, not cryptographically attested.
