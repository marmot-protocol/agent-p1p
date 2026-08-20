# Rust worker result contracts

**Status:** Canonical for workflow version 2 and contract version 1

The Rust controller accepts a completed worker result only from the latest
successful durable Hermes run for a controller-owned task projection. The
stored projection, not the worker result, supplies the immutable assignment.
The controller rejects task, role, profile, case, plan, PR, head, model, or
skills-commit drift before writing a run or advancing a case.

Every controller-owned worker projection also carries
`immutable_evidence_bundle` schema version 1. It is bound to the task's case
and state revision and contains the deterministically ordered immutable ledger
events, runs, controller evidence, and findings available when the dispatch
effect is claimed. Each record retains its stored payload digest. The top-level
digest is SHA-256 over the compact, lexicographically key-ordered JSON bundle
after removing only the top-level `sha256` field. Workers fail closed on a
missing or mismatched bundle; the result contract does not echo the bundle.
For final review, the atomically preceding `GITHUB_FINAL_PREFLIGHT` record also
contains the freshly fetched issue title/body, bounded issue comments and body
digests, trusted authorization event, PR, complete CI history, published
reviews, and review threads.

The executable definitions are in `crates/pip-contracts/src/lib.rs`. Valid
examples for every role are frozen in
`migration/target-v1/worker-results.json`. This document is the worker-facing
field guide; it does not replace executable validation.

## Common fields

Every result is one JSON object with exactly these common fields plus the
role-specific fields below:

| Field | Contract |
|---|---|
| `contract_version` | Integer `1`. |
| `workflow_version` | Positive integer equal to `case.workflow_version`. |
| `case` | Object containing positive `repository_id`, `issue_number`, and `workflow_version`. |
| `task_id` | Exact Hermes task ID from the immutable task input. |
| `role` | `planner`, `builder`, `reviewer-general`, `reviewer-secperf`, or `final-reviewer`. |
| `requested_model` | Exact `provider/model` value from the task binding. |
| `actual_model` | Model identity observed by the worker. A mismatch requires `BLOCKED_UNEXPECTED_MODEL`. |
| `skills_repository_commit` | Exact lowercase 40-hex commit from the task binding. |
| `started_at_unix` | Nonnegative Unix timestamp in seconds. |
| `completed_at_unix` | Unix timestamp in seconds, not earlier than start. |
| `evidence` | JSON object containing attributable evidence and durable artifact paths. |

Unknown top-level fields are rejected. Return timestamps as integers, not ISO
strings. Put supplemental diagnostics, artifact paths, confidence, and tool
limitations under `evidence`; do not invent top-level fields.

## Planner

Additional fields:

- `outcome`: `PROCEED`, `ALREADY_FIXED`, `NOT_REPRODUCIBLE`, `DUPLICATE`,
  `ROOT_CAUSE_DIFFERENT_SCOPE`, `CROSS_REPO_DEPENDENCY`,
  `WAITING_FOR_ISSUE_CREATOR`, `NEEDS_HUMAN_SCOPE_DECISION`, `ABANDON`,
  `BLOCKED`, or `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version`;
- lowercase 40-hex `planned_base_sha`;
- `root_cause`, `authorized_scope`, `sensitive_scope`, `dependencies`,
  `open_decisions`, and `plan_artifact`.

`PROCEED` requires empty sensitive scope, dependencies, and open decisions.
The planner never publishes or identifies a GitHub comment. The controller
renders the accepted contract into an immutable provenance-marked issue comment
and records its numeric ID and body digest as controller evidence before any
builder or human-disposition effect is released.

## Builder

Additional fields:

- `outcome`: `REVIEW_READY`, `RETURN_TO_PLANNING`, `BLOCKED`, `ABANDON`, or
  `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version` and `build_round`;
- nullable `head_sha`;
- `local_checks`; and
- `finding_resolutions` with exact finding and resolution-head bindings.

`REVIEW_READY` requires the lowercase 40-hex local commit on the task's exact
`assigned_branch` in its `assigned_worktree`. The clean builder process has no
GitHub credential and must not push. The controller uses the accepted head and
the ledger's prior remote head in an exact force-with-lease transaction. It
requires the policy-bound push URL and ignores repository hooks, filesystem
monitors, credential helpers, proxies, and HTTP headers while forcing TLS
verification. After verifying the resulting remote SHA, it creates or updates
the case-owned draft PR, binds its numeric identity and exact head in the
ledger, and independently reads the complete GitHub CI attempt history before
releasing reviewers.

## Reviewers

Additional fields:

- `outcome`: `APPROVE`, `REQUEST_CHANGES`, `BLOCKED`, or
  `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version`, `review_round`, and `pr_number`;
- lowercase 40-hex `reviewed_head_sha`;
- `blocking_findings`, `suggestions`, and `finding_confirmations`.

An approval cannot contain a blocking finding or an open confirmation. The
first independent result is retained without advancing the case. The second
same-head, same-round result releases the deterministic aggregate verdict.

## Final reviewer

Additional fields:

- `outcome`: `READY`, `RETURN_TO_BUILD`, `RETURN_TO_REVIEW`,
  `RETURN_TO_PLANNING`, `WAIT_FOR_ISSUE_CREATOR`, `BLOCKED`, `ABANDON`, or
  `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version`, `final_review_round`, and `pr_number`;
- lowercase 40-hex `reviewed_head_sha`;
- `residual_uncertainties`; and
- nonempty `decision_rationale`.

`READY` is a recommendation, not merge authority. In MDK shadow policy the
pure state machine maps it to `SHADOW_READY`, where disposition remains held
for a human.

## Hermes completion envelope

After validating the result locally, call `kanban_complete` once with a concise
summary and the complete contract object as run `metadata`. Return the same
object as the entire final response, without prose or a code fence. Hermes may
store its own run envelope fields outside `metadata`; do not add those fields
to the contract object.
