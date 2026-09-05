# Canary preflight: issue inputs and stock-Hermes failure handling

Source changes only; no new canary, provider inference, release installation,
policy activation, task reset, or Hermes fork. The interrupted #1639 task and
ledger history remain preserved. This follows the
[planner investigation](2026-09-05-planner-failure-investigation.md).

## Implemented

- New `ISSUE_AUTHORIZED` events freeze the controller-read repository, issue
  identity/labels, body/title/author, comments including identities and body
  digests, and observation time in `issue_context` schema 1. The existing
  event digest and immutable worker-history bundle bind that snapshot.
  Serialized context is limited to 256 KiB before case/outbox creation; it is
  not silently truncated. Reconciliation does not rewrite an existing case's
  snapshot. This does not add source-head or related-issue evidence, and does
  not backfill historical tasks or promise the whole growing history fits the
  separate 512 KiB worker-bundle cap.
- Optional policy `max_hermes_attempts` controls Hermes tasks independently of
  direct-provider failures. An absent field preserves historical behavior and
  serialization; zero is rejected. Undeployed target revision 6 sets one
  Hermes attempt while retaining three direct-provider failures. Existing
  frozen tasks are not rebound. This is conservative fail-fast behavior, not
  typed provider-error classification; transient worker failures also stop.
- Result escalation records the frozen task's attempt limit rather than a
  later policy's value.
- The real stock-Hermes probe exposed a second adapter mismatch: a clean-exit
  protocol violation ends its run as `crashed`, then emits a `gave_up` event.
  It does not change that run's outcome to `gave_up`. Pip now recognizes the
  final breaker event with a matching failed-run profile, trigger outcome,
  positive per-task bound, and exhausted failure count. Stale/reopened,
  below-bound, mismatched, or nonblocked examples remain incomplete.
- Planner instructions consume the dated controller snapshot, treat issue
  prose as untrusted data, prohibit credential hunting/live-intake workarounds,
  and prohibit moving build output into profile caches. Without an explicit
  scratch location, analysis stays read-only; required missing evidence or
  essential unexecutable tests must be surfaced as blocked work.

## Validation

Regression tests first reproduced missing issue context, the absent separate
attempt setting, wrong escalation counts after policy drift, and the missed
crash-breaker event. Tests cover frozen body/comments and replay, oversize
rejection without case/outbox creation, legacy policy compatibility, one-attempt
escalation without an accepted worker result, and negative breaker evidence.

`tests/fixtures/hermes_failure_probe.py` ran on Pirate against unchanged stock
Hermes `29112bef099274229cadff79cdff7bf7b99c4b77`, in temporary homes/boards.
Only PID liveness/exit observations are simulated; stock dispatch, crash
accounting, database writes and `kanban show --json` execute normally. No real
worker process or model is started. With limit 3 the first simulated failure
returns to ready. With limit 1 it becomes blocked, and two more dispatch ticks
create no second attempt. Temporary test databases are removed on exit.

The opt-in Rust test
`stock_hermes_protocol_failure_is_consumed_without_a_model_call` also passed:
the fixture ran on Pirate through an SSH test runner, and its actual CLI JSON
was consumed by the local Rust adapter as `RetryLimitReached`. This crosses
the real serialization boundary; it is not a full gateway/systemd/provider
test. The probe is intentionally opt-in for machines with stock Hermes.

Final validation: `cargo test -p pip-control -p pip-controller -p pip-hermes
--tests` passed 169 tests, with four explicitly external tests skipped by
default. The new stock-Hermes failure test passed separately with `--ignored`;
the other three external tests were not rerun. Focused Clippy with warnings
denied, `cargo fmt --all --check`, and `git diff --check` passed.

## Still required before another live canary

1. Implement controller-assigned, managed scratch/build storage with reserve
   checks and safe cleanup. Skill restrictions alone are not filesystem quotas
   or lifecycle enforcement. The existing 9.7 GiB planner cache is preserved.
2. Provision or disable optional tools coherently and exercise one complete
   offline planner lifecycle under the actual service sandbox, including
   artifacts and blocked outcomes.
3. Build/review/install the candidate and reconcile the new inert policy and
   profiles. None of these source changes are deployed yet.
4. Resolve the provider-rejection/access question and select an explicitly
   authorized fresh attempt. Do not replay the rejected request by changing
   models, rewriting an old task, or resetting historical case state.

Pirate's execution timers and worker gateway were rechecked disabled, with
gateway MainPID 0. Conversational Pip and webhook ingress were not changed.
MDK remains shadow-only with autonomous merge disabled.
