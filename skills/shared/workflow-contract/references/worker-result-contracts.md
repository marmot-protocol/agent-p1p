# Rust worker result contracts

This field guide is a resource of the `workflow-contract` skill. Resolve it
relative to that loaded skill's directory, not the repository being worked on.

**Status:** Canonical for workflow version 3 and contract version 2

The Rust controller accepts results from durable Hermes runs or the direct
Cursor queue, bound to a controller-owned task. The stored assignment, not
the worker result, supplies the immutable task identity.
The controller rejects task, role, profile, case, plan, PR, head, model, or
skills-commit drift before writing a run or advancing a case.
The job's PR/head describe incoming context for planners and builders; they
are not output fields a builder must echo. A remediation builder returns its
new commit in `head_sha`. The controller checks that commit in the assigned
checkout before publishing it to the existing PR. Review and final-review
results, by contrast, must name the exact PR/head they were assigned to review.

Every controller-owned worker projection supplies
`immutable_evidence_bundle` schema version 1 or 2, inline or through
`immutable_evidence_ref` (`schema_version: 1`, absolute `path`, `sha256`). For
a reference, first verify the file's SHA-256 over its exact bytes (for example
with `sha256sum`); parse the file as the bundle only after that matches.
The bundle is bound to the task's case
and state revision and contains the deterministically ordered immutable ledger
events, runs, controller evidence, findings, and completed detached review
observations available when the dispatch
effect is claimed. Each record retains its stored payload digest. The top-level
digest is SHA-256 over the compact, lexicographically key-ordered JSON bundle
after removing only the top-level `sha256` field. Workers fail closed on a
missing or mismatched bundle; the result contract does not echo the bundle.
Version 2 avoids duplicate accepted-result payloads: an event can contain
`payload_ref: {"run_id": "…", "payload_sha256": "…"}` instead of `payload`.
Find that exact `run_id` in `records.runs`, require its digest to match both the
reference and the event's `payload_sha256`, and use the run's `payload`.
All records remain present; only identical copies of payloads are replaced.
Version 1 retains inline event payloads and remains valid for saved jobs.
For final review, the atomically preceding `GITHUB_FINAL_PREFLIGHT` record also
contains the freshly fetched issue title/body, bounded issue comments and body
digests, trusted authorization event, PR, complete CI history, published
reviews, and review threads.

Verify digests using a local program without printing all payloads into the
conversation. Then inspect relevant records: planners use issue/intake and
replanning evidence; builders use the accepted plan and applicable findings;
reviewers use the plan, accepted build, exact-head CI and their prior findings;
final reviewers also inspect final preflight and the required review set.
New jobs also supply `evidence_focus` (`schema_version: 1`, `records` array).
Each entry has a JSON `pointer` into this verified bundle and the selected
record's `payload_sha256`. Resolve that pointer with a local program, verify
the record digest matches, and read its relevant fields without printing
unrelated logs. The index selects the current plan; builders also get prior
build/reviews and findings; reviewers get the bound build/CI and their own
findings; final reviewers also get the accepted review set and preflight.
Reviewer indexes omit peer verdicts to support independent review. They do not
make those verdicts secret. Full history remains available in the artifact.
The index is a reading aid, not a completeness or readiness attestation. Do
not treat absence from the index as absence from evidence. Saved jobs without
an index remain valid: select the relevant records as described above.

The executable definitions are in `crates/pip-contracts/src/lib.rs`. Valid
examples for every role are frozen in
`migration/target-v1/worker-results.json`. This document is the worker-facing
field guide; it does not replace executable validation.

## Common fields

Every result is one JSON object with exactly these common fields plus the
role-specific fields below:

| Field | Contract |
|---|---|
| `contract_version` | Integer `2`. |
| `workflow_version` | Positive integer equal to `case.workflow_version`. |
| `case` | Object containing positive `repository_id`, `issue_number`, and `workflow_version`. |
| `task_id` | Exact controller-assigned task ID from the immutable task input. |
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

Stock Hermes may add `worker_session_id`, `artifacts`, and `_staged_artifacts`
to its durable completion metadata. Pip's Hermes adapter validates and separates
only these known transport annotations before strict contract decoding; it
retains the original metadata unchanged. They do not provide authorization,
model attestation, or proof that an artifact exists. Other unknown fields still
fail closed. Workers should not add these fields to their contract JSON.

## Planner

Additional fields:

- `outcome`: `PROCEED`, `ALREADY_FIXED`, `NOT_REPRODUCIBLE`, `DUPLICATE`,
  `ROOT_CAUSE_DIFFERENT_SCOPE`, `CROSS_REPO_DEPENDENCY`,
  `WAITING_FOR_ISSUE_CREATOR`, `NEEDS_HUMAN_SCOPE_DECISION`, `ABANDON`,
  `BLOCKED`, or `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version`;
- lowercase 40-hex `planned_base_sha`;
- string `root_cause`, `authorized_scope`, and `plan_artifact`;
- arrays `sensitive_scope` (category strings), `dependencies` (JSON values),
  and `open_decisions` (strings). Empty lists mean `[]`, not `null`.

Sensitive categories are `CRYPTOGRAPHY`, `MLS_CGKA`, `KEY_HANDLING`,
`TRUST_ANCHOR`, `MEMBERSHIP_AUTHORIZATION`, `ADMIN_AUTHORIZATION`, and
`PUSH_PAYLOAD_CONTEXT`.

Tasks with `storage` schema 1 or 2 also require the full plan in
`evidence.plan_markdown`: nonempty UTF-8 text, at most 16 KiB. This is the
digest-bound handoff to direct workers that cannot read private Hermes files.
Keep a retained file copy under `storage.results`; do not rely on its path as
the only implementation plan.

`PROCEED` requires empty sensitive scope, dependencies, and open decisions.
The planner never publishes or identifies a GitHub comment. The controller
renders the accepted contract into an immutable provenance-marked issue comment
and records its numeric ID and body digest as controller evidence before any
builder or human-disposition effect is released.

## Builder

Additional fields:

- `outcome`: `REVIEW_READY`, `RETURN_TO_PLANNING`, `BLOCKED`, `ABANDON`, or
  `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version` and `build_round`; copy the task's explicit
  `build_round`. For a saved job without that field, it is
  `remediation_round + 1` (initial build 1, first remediation 2). A retry does
  not itself increment the remediation round. Historical task IDs may contain
  the old ambiguous round label; do not infer the result field from that label;
- nullable `head_sha`;
- `local_checks`: an array of strings describing commands and outcomes, such as
  `["cargo test -p example: 42 passed", "cargo clippy: passed"]`. Never an object
  like `{"passed": true}` or a boolean. Report failed checks and limitations
  honestly; detailed structured command records may go inside `evidence`;
- `finding_resolutions`: an array (empty for an initial build). Each object has
  exactly string fields `finding_id`, `resolution_commit`, `resolved_head_sha`,
  `resolution_summary`, and `tests` as an array of strings.

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

- `reviewer_id`: exact stable reviewer-instance identity from the task binding;
- `outcome`: `APPROVE`, `REQUEST_CHANGES`, `BLOCKED`, or
  `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version`, `review_round`, and `pr_number`;
- lowercase 40-hex `reviewed_head_sha`;
- `blocking_findings`: an array of objects with exactly string fields `id`,
  `summary`, `defect`, `consequence`, `corrective_direction`, plus
  `required_evidence` as an array of strings;
- `suggestions`: an array of objects with exactly string fields `summary` and
  `rationale`;
- `finding_confirmations`: an array of objects with exactly string `finding_id`,
  `status` (`CONFIRMED_RESOLVED` or `STILL_OPEN`), string `reviewed_fix_sha`, and
  `evidence` as an array of strings. Empty lists are `[]`, not `{}` or `null`.

An approval cannot contain a blocking finding or an open confirmation. The
controller-owned task binding also carries `review_mode`: `required`,
`advisory`, or `shadow`; workers cannot select or change it. Required results
are retained without advancing until every policy-required instance has
returned on the same head and round. The deterministic union then releases the
aggregate verdict. Advisory and shadow results are stored as immutable
observations and never advance, block, approve, or remediate the case.

## Final reviewer

Additional fields:

- `outcome`: `READY`, `RETURN_TO_BUILD`, `RETURN_TO_REVIEW`,
  `RETURN_TO_PLANNING`, `WAIT_FOR_ISSUE_CREATOR`, `BLOCKED`, `ABANDON`, or
  `BLOCKED_UNEXPECTED_MODEL`;
- positive `plan_version`, `final_review_round`, and `pr_number`;
- lowercase 40-hex `reviewed_head_sha`;
- `residual_uncertainties`: an array of strings (empty means `[]`); and
- nonempty string `decision_rationale`.

`READY` is a recommendation, not merge authority. In MDK shadow policy the
pure state machine maps it to `SHADOW_READY`, where disposition remains held
for a human.

## Validate before completion

Save the JSON contract outside the source checkout: under `storage.results`
for Hermes, or the run artifact directory supplied by the direct runtime.
Run `/opt/pip/current/bin/pip-control validate-worker-result --input /absolute/path/worker-result.json`.
Correct any field/type errors before submitting. This credential-free command
checks the actual Rust schema and self-consistency; it does not authorize work,
accept a job result, validate claimed tests, or grant CI/merge readiness.

## Completion transport

For Hermes tasks, after validation call `kanban_complete` once with a concise
summary and the complete contract object as run `metadata`. Return the same
object as the entire final response, without prose or a code fence. Hermes may
store its own run envelope fields outside `metadata`; do not add those fields
to the contract object.

For direct Cursor tasks, return the contract as the final JSON object. The direct
runtime captures it; do not look for Hermes tools or update Kanban yourself.
