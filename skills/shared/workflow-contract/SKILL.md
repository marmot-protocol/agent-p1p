---
name: workflow-contract
description: Use for every Pip case task. Enforce shared invariants.
version: 0.14.0
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
6. Require the bound evidence bundle, either inline as `immutable_evidence_bundle` or in the artifact named by `immutable_evidence_ref`. For a reference, verify the file's exact byte SHA-256 before parsing it; then verify the bundle's schema version 1 or 2, `case_key`, `bound_state_revision`, and internal `sha256` as described in the field guide. In version 2, an event may use `payload_ref` to reference its identical accepted run payload; resolve the run ID and matching digest rather than treating the payload as missing. Start with the task's role-specific `evidence_focus` index when present, resolving its JSON pointers inside this verified bundle. Expand into other records when needed; the index is a reading aid, not proof that other evidence is absent. Never dump the entire history into context. Missing, malformed, oversized, or mismatched evidence blocks completion; never replace it with session memory or a parent summary.
7. Bind CI and review evidence to an exact 40-character PR head SHA.
8. Do not treat CodeRabbit as mandatory; concrete findings are still actionable. If a CodeRabbit status exists but says the review was rate limited, do not represent it as complete evidence.
9. Under the current strict CI policy, a failed attempt on the exact reviewed head blocks acceptance even after a green rerun. A new head requires fresh CI and reviews; do not treat a failure on an older head as a permanent ban on the PR. The controller owns this deterministic gate and supplies the GitHub evidence; workers do not need GitHub credentials or independently administer authorization.
10. Do not silently broaden scope or edit a dependency repository.
11. Human takeover or removed authorization stops the case.
12. Complete the versioned structured result contract before reporting success.
13. Never merge directly from a planning, building, or review role.
14. Worker processes never push Git branches or receive GitHub credentials. A builder commits only in its exact `assigned_worktree` on `assigned_branch`; the deterministic controller publishes and verifies that branch after accepting the result.
    The assigned checkout has case-local Git metadata; do not replace `.git`, add object alternates, change remotes, install hooks/filters, or add Git config includes or URL rewrites. Use command-scoped author/committer settings (or local `user.name`/`user.email`) for commits. Do not change the inherited exact-workspace Git safety settings. If a repository needs additional Git configuration, report the requirement instead of altering the execution boundary.
15. Parent summaries may be truncated. Hermes workers resolve declared parents on their assigned board and read durable run metadata. Direct workers use the bound evidence bundle and accessible retained artifacts, not unavailable Hermes tools. Never rely on a truncated summary for PR numbers, findings, or remediation evidence.
16. Return contract version 2 with exactly these common fields plus the role fields: `contract_version`, `workflow_version`, `case` (`repository_id`, `issue_number`, `workflow_version`), `task_id`, `role`, `requested_model`, `actual_model`, `skills_repository_commit`, integer `started_at_unix`, integer `completed_at_unix`, and object `evidence`. Review results also copy the exact `reviewer_id`; the controller-owned `review_mode` binding is not an output choice. Put supplemental artifact paths or diagnostics inside `evidence`. The full field guide is `references/worker-result-contracts.md` in the loaded `workflow-contract` skill directory (not the target repository).

## Hermes storage

For Hermes tasks carrying `storage` schema 1, 2 or 3, `source` is the controller-owned
read-only checkout, and the current directory remains that checkout. Use the
exact `cargo_target`, `cargo_home`, and `temporary` paths from the task as
`CARGO_TARGET_DIR`, `CARGO_HOME`, and `TMPDIR` for Cargo commands. The controller
has already created these paths. Set all three variables on every Cargo command;
do not assume shell exports persist across terminal tool calls. Keep plan/review artifacts in `results`, not
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
