# Pip implementation status

Updated 2026-09-06. This is a work inventory, not an activation or completion claim.
The target is [the lean architecture](pip-architecture-plan.md).

## Current live evidence

Pirate was checked during this refactor on 2026-09-06:
- installed source: `c1f70e7d61050e175e9c2723d78d66c2a001086e`;
- the Cursor argument-boundary repair is installed and execution has resumed;
- #993 is the sole authorized canary; planner task `t_bf698ea6` completed and
  plan version 1 was accepted. The first Cursor invocation rejected skill
  frontmatter as a command-line option before useful work. Attempt 7 is executing
  on `c1f70e7`, with source edits and Rust compilation observed;
- #891, #1228 and #1639 are abandoned with their history retained;
- no completed end-to-end issue or ready PR;
- Hermes remains upstream commit `29112bef099274229cadff79cdff7bf7b99c4b77`,
  with no local source modifications.

These are dated observations, not guarantees about a later host state.
Recheck before installation/activation. Conversational Hermes is separate from
the service-owned execution runtime and must not be interrupted.

## Lean refactor in progress

Implemented and deployed through `cd738ee`:

- Replace the long architecture specification with the approved smaller scope.
  Keep webhooks and polling, Rust workflow authority, unmodified Hermes,
  explicit models and human-only merge.
- Exclude detached review failures from the case work budget while retaining
  every attempt in history and aggregate status.
- Report failed workspace retirement without blocking otherwise healthy storage
  or falsely recording retirement.
- Remove the alternate in-process direct execution path. Its useful tests now
  exercise the production controller/inbox/worker/result boundary.
- Recover dispatch from saved task definitions when release/profile defaults
  change, rather than reconstructing and conflicting with the original job.
- Acquire reviewer-App credentials only when publishing the relevant review,
  not at the beginning of every controller cycle.
- Permit Node/V8 JIT memory in the execution services, retaining controller and
  ingress restrictions. The lifecycle fixture must actually execute rather than
  silently skip a missing Hermes bootstrap marker.
- Record confirmed direct-runtime non-starts separately from task failures and
  retry with durable exponential backoff. Replay and restart preserve the delay;
  an uncertain queue handoff remains fenced rather than authorizing a duplicate.

- Verify effective Hermes profile settings through `config get --json`, not
  presentation-sensitive YAML text comparisons. Exact model/provider checks stay.
- Remove the autonomous-merge coordinator and GitHub merge-write API. Current
  policies must be shadow/human-only; historical state remains readable.
- Stop setting unnecessary setgid bits on case and artifact directories. Both
  execution identities already share the same primary group. Preserve
  `RestrictSUIDSGID=yes` and verify controller preparation inside its sandbox,
  not only the later worker handoff.

Deployed additionally through `c1f70e7`:

- Delimit the Cursor positional prompt with `--` so Markdown frontmatter cannot
  become command-line options. The regression failed before the fix and passes
  after it; the installed Cursor accepts the delimiter in an offline probe.
- Seed paused policy only on first install; validate and preserve existing
  operator configuration during upgrades/reinstalls. Invalid existing policy
  stops installation instead of silently replacing state with defaults.

Regression tests reproduce the shadow-budget, cleanup and saved-dispatch defects
before the fixes. The full Rust workspace tests and Clippy pass locally. Linux
lifecycle verification passed clean install, reinstall, upgrade, rollback,
restart recovery, two-UID workspace handoff and both execution-service JIT
boundaries. After the live #993 reproduction, the expanded controller-preparation
fixture and full Linux lifecycle passed with `RestrictSUIDSGID=yes` retained.
The permission repair is deployed and dispatched the same retained #993 case.
The policy-preserving installer passed 329 Rust tests (seven explicitly ignored),
Clippy and the expanded Linux lifecycle, including preservation of active-policy
bytes through reinstall, upgrade, rollback and reboot with execution disabled.
The installer change is now deployed and preserved exact live policy and ledger
bytes on Pirate. End-to-end proof remains outstanding.

Further local changes awaiting release:

- Pass the retained Cursor prompt file through stdin, avoiding Linux's
  single-argument length bound and keeping prompt content out of process arguments.
  Large-input regressions, full Rust tests and Clippy pass; installed Cursor
  accepted both pipe input and a regular stdin file in isolated no-tool probes.
- Read-only `status --case` and `status --attempt` expose retained history and
  failure details without handwritten SQL. CLI regression tests and Clippy pass.

## Remaining work toward the active goal

1. Extend confirmed-non-start handling to remaining service/bootstrap failures;
   direct-runtime probe failures now have bounded backoff without consuming the
   case's work-failure budget.
2. Finish failure isolation: one publication/capability failure cannot stall
   unrelated cases; safe result processing must remain possible during pause.
3. Complete saved-job recovery through execution/result acceptance and compatible
   policy/settings upgrades, not just queue reconciliation. Preserve stale-job,
   revocation and exact-model guards.
4. Replace full-history prompt replication with bounded role-specific inputs and
   retained immutable evidence. Remove formatting-sensitive Hermes assumptions.
5. Simplify storage/maintenance and normal operating commands; retire obsolete
   runtime paths and defer automatic merge code without losing historical reads.
6. Verify release and real service-identity execution, then select/authorize one
   issue and run planner, builder, CI, required independent reviewers,
   remediation if needed, and final review to a human-ready PR.
7. Recheck exact PR head, required reviews and CI; do not merge.
8. After the cutover proof, remove the legacy Python runtime and obsolete CI/docs,
   preserving useful parity fixtures and source history in Git.
9. Reconcile remaining documentation and remove temporary operational scaffolding.
   Revoke temporary operator elevation when no longer needed.

Keep the complete goal active until both the slimmed architecture and live PR
are verified. Tests of adapters, a healthy process, or a successful planner alone
do not establish an end-to-end success.

## Historical evidence

`docs/evidence/` contains dated deployment and canary records. The older
[completion audit](completion-audit.md), Python inventory and migration roadmap
are historical references; their installed-source/activation snapshots are not
current state. Do not infer permission or recovery commands from those snapshots.
