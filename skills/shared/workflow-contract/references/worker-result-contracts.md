# Worker result field guide

This guide is a resource of the `workflow-contract` skill. Resolve it relative
to that loaded skill's directory, not the repository being worked on.

You return one JSON object describing your work. Pip already knows the job
you were given, so it fills in the bookkeeping itself; you only report what
you decided and found.

## Common fields

Pip fills these from the task binding. You may omit them; if you include them,
Pip overwrites them with the binding's values:

`contract_version`, `workflow_version`, `case`, `task_id`, `role`,
`requested_model`, `actual_model`, `skills_repository_commit`, `plan_version`,
`pr_number`, `reviewer_id`, and your round (`build_round`, `review_round` or
`final_review_round`).

Pip records start and completion times from the run itself. You may report
integer Unix `started_at_unix` and `completed_at_unix`; implausible values are
replaced.

`evidence` is an object for anything worth keeping that has no field of its
own: commands you ran, artifact paths, limitations, confidence. Unknown
top-level fields are ignored, so put useful material in `evidence` rather than
inventing new fields.

Lists are arrays (`[]` when empty), never `null` or objects. SHAs are lowercase
40-character hex.

## Planner

- `outcome`: `PROCEED`, `ALREADY_FIXED`, `NOT_REPRODUCIBLE`, `DUPLICATE`,
  `ROOT_CAUSE_DIFFERENT_SCOPE`, `CROSS_REPO_DEPENDENCY`,
  `WAITING_FOR_ISSUE_CREATOR`, `NEEDS_HUMAN_SCOPE_DECISION`, `ABANDON`, or
  `BLOCKED`.
- `planned_base_sha`: the checkout HEAD you planned against.
- `root_cause`, `authorized_scope`, `plan_artifact`: strings.
- `sensitive_scope`: categories the change touches, from `CRYPTOGRAPHY`,
  `MLS_CGKA`, `KEY_HANDLING`, `TRUST_ANCHOR`, `MEMBERSHIP_AUTHORIZATION`,
  `ADMIN_AUTHORIZATION`, `PUSH_PAYLOAD_CONTEXT`. Listing them does not stop a
  plan; it tells reviewers where to look hardest.
- `dependencies`: array of objects; `open_decisions`: array of strings.
  `PROCEED` requires both to be empty. If a real product decision is open,
  use `NEEDS_HUMAN_SCOPE_DECISION` and name it.

Tasks with a `storage` object also put the full plan in
`evidence.plan_markdown` (nonempty, at most 16 KiB). Pip publishes it on the
issue and hands it to the builder.

## Builder

- `outcome`: `REVIEW_READY`, `RETURN_TO_PLANNING`, `BLOCKED`, or `ABANDON`.
- `head_sha`: your commit on `assigned_branch` when `REVIEW_READY`; omit it
  otherwise.
- `local_checks`: strings describing commands and their results, for example
  `["cargo test -p example: 42 passed", "cargo clippy: passed"]`. Report
  failures honestly.
- `finding_resolutions`: one object per finding you addressed, with string
  `finding_id`, `resolution_commit`, `resolved_head_sha`, `resolution_summary`
  and `tests` (array of strings). Empty for an initial build.
- `evidence.pr_title`, `evidence.problem_summary`, `evidence.solution_summary`:
  short strings Pip uses for the PR description.
- On remediation, `evidence.suggestion_dispositions`: for each reviewer
  suggestion you considered, the reviewer, the suggestion, `addressed` or
  `deferred`, and a `summary` of why.

Do not push. The controller signs your commit, publishes the branch and
updates the draft PR.

## Reviewers

- `outcome`: `APPROVE`, `REQUEST_CHANGES`, or `BLOCKED`.
- `reviewed_head_sha`: the exact head you reviewed. Native (Hermes) reviewers
  must report it; it is how Pip knows what you examined.
- `blocking_findings`: objects with string `id`, `summary`, `defect`,
  `consequence`, `corrective_direction`, and `required_evidence` (array of
  strings).
- `suggestions`: objects with string `summary` and `rationale`. Suggestions
  never block.
- `finding_confirmations`: for each earlier finding of yours, string
  `finding_id`, `status` (`CONFIRMED_RESOLVED` or `STILL_OPEN`), string
  `reviewed_fix_sha`, and `evidence` (array of strings).

`APPROVE` cannot carry blocking findings or open confirmations.

## Final reviewer

- `outcome`: `READY`, `RETURN_TO_BUILD`, `RETURN_TO_REVIEW`,
  `RETURN_TO_PLANNING`, `WAIT_FOR_ISSUE_CREATOR`, `BLOCKED`, or `ABANDON`.
- `reviewed_head_sha`: the exact head you reviewed.
- `residual_uncertainties`: array of strings.
- `decision_rationale`: a nonempty explanation.

`READY` is a recommendation to a human, never merge authority.

## Check your result

Save the object outside the source checkout and run:

- direct (Cursor) tasks:
  `/opt/pip/current/bin/pip-control validate-worker-result --input <file> --task-input <run artifact dir>/task-input.json`
- Hermes tasks:
  `/opt/pip/current/bin/pip-control validate-worker-result --input <file> --role <your role>`

Fix whatever it reports. It checks the shape of your fields; it does not
authorize or accept anything.

## Completion transport

Hermes tasks: call `kanban_complete` once with a short summary and the result
object as run `metadata`. If you cannot do the job at all, return a `BLOCKED`
result with the reason in `evidence`; do not leave the task blocked in Kanban.

Direct tasks: save the object as `worker-result.json` in the run artifact
directory. Pip reads that file; your final message is only a fallback.
