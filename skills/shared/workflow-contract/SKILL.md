---
name: workflow-contract
description: Use for every Pip case task. Enforce shared invariants.
version: 0.6.0
author: agent-p1p
license: MIT
metadata:
  hermes:
    tags: [pip, workflow, contracts, exact-head]
    related_skills: []
---

# Pip Workflow Contract

## Overview

This is the shared contract for every Pip role. Role-specific skills add responsibilities but may not weaken these invariants.

## Invariants

1. Start from durable case artifacts and current source state. Do not rely on prior session memory.
2. Work only on the assigned repository, issue, case, and Pip-owned branch.
3. Never expose credentials or secrets in output, logs, comments, or artifacts.
4. Record requested and actual models. If they differ, return `BLOCKED_UNEXPECTED_MODEL`.
5. Copy the case identity, task ID, role, reviewer instance ID when present, plan version, requested `provider/model`, skills repository commit, PR number, and expected head exactly from the immutable task binding. Never reconstruct or normalize them from prose.
6. Require `immutable_evidence_bundle` schema version 1. Verify its `case_key` and `bound_state_revision` equal the task binding, and verify its `sha256` over the compact, lexicographically key-ordered JSON object after removing only the top-level `sha256` field. Treat every record payload and stored `payload_sha256` as immutable input. Missing, malformed, oversized, or mismatched evidence is a blocked result; never replace it with session memory or a parent summary.
7. Bind CI and review evidence to an exact 40-character PR head SHA.
8. Do not treat CodeRabbit as mandatory; concrete findings are still actionable. If a CodeRabbit status exists but says the review was rate limited, do not represent it as complete evidence.
9. A PR that had any red CI attempt is permanently ineligible. A green rerun does not clear that history.
10. Do not silently broaden scope or edit a dependency repository.
11. Human takeover or removed authorization stops the case.
12. Complete the versioned structured result contract before reporting success.
13. Never merge directly from a planning, building, or review role.
14. Worker processes never push Git branches or receive GitHub credentials. A builder commits only in its exact `assigned_worktree` on `assigned_branch`; the deterministic controller publishes and verifies that branch after accepting the result.
15. Parent summaries may be truncated. Resolve every declared parent on the task's assigned board, read the full durable run metadata, and dereference declared result artifacts before relying on PR numbers, findings, or remediation evidence.
16. Return contract version 2 with exactly these common fields plus the role fields: `contract_version`, `workflow_version`, `case` (`repository_id`, `issue_number`, `workflow_version`), `task_id`, `role`, `requested_model`, `actual_model`, `skills_repository_commit`, integer `started_at_unix`, integer `completed_at_unix`, and object `evidence`. Review results also copy the exact `reviewer_id`; the controller-owned `review_mode` binding is not an output choice. Put supplemental artifact paths or diagnostics inside `evidence`. The full field guide is `references/worker-result-contracts.md` in the loaded `workflow-contract` skill directory (not the target repository).

## Hermes storage

For Hermes tasks carrying `storage` schema 1, `source` is the controller-owned
read-only checkout, and the current directory remains that checkout. Use the
exact `cargo_target`, `cargo_home`, and `temporary` paths from the task as
`CARGO_TARGET_DIR`, `CARGO_HOME`, and `TMPDIR` for Cargo commands. The controller
has already created these paths. Keep plan/review artifacts in `results`, not
in disposable build directories. Never redirect builds into profile caches,
operator homes, or another task's storage. Do not install language servers.
If paths are absent, unwritable, or the disk reserve is exhausted, block and
report the failure; do not invent substitute paths. These rules do not alter
direct-worker storage or allow a planner/reviewer to modify source.

Managed-storage planners must include the full canonical plan as
`evidence.plan_markdown` (nonempty, at most 16 KiB of UTF-8). Keep the file in
`storage.results` as a retained copy. The accepted run and its payload digest,
not cross-user filesystem access, bind the plan for downstream workers.
Builders and reviewers read this field from the accepted planner run in the
immutable evidence bundle. Private artifact paths are provenance, not a
requirement to bypass their sandbox. The controller also publishes the inline
plan in the issue comment before builder dispatch.

## Ownership

Only Pip-authored `pip/*` work is eligible. Existing human-owned PRs and human-held cases fail closed. Technical access is not authorization.

## Completion

A run is complete only when its durable artifacts exist, its JSON result validates, and all claimed evidence can be fetched independently.

## Common pitfalls

- Reusing an earlier CI result after the head changed.
- Calling a finding resolved before the originating reviewer confirms it.
- Guessing product intent from code.
- Treating an external reviewer outage as approval.
- Returning prose without the required structured result.

## Verification checklist

- [ ] Scope and authorization are current.
- [ ] Requested and actual models match.
- [ ] Every SHA-specific claim references the current SHA.
- [ ] Output validates against the role schema.
- [ ] No secrets or unrelated repository changes appear.
