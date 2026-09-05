# Isolated real planner and Rust result acceptance

Status: one real planner completion accepted by the candidate Rust controller;
production remains paused. No Hermes fork, production deployment, live issue
authorization, GitHub publication, or full-pipeline claim.

## Exact execution

- Host: Pirate, 2026-09-05, approximately 08:31–08:36 UTC.
- Stock Hermes: `29112bef099274229cadff79cdff7bf7b99c4b77`, unchanged.
- Candidate dispatch and skills: `1b056fcc5f032f9dfeec3acb5e986dde13dd46dd`.
- Model: `openai-codex/gpt-5.6-sol`, profile reasoning `xhigh`, no fallbacks.
  The Hermes session database independently records this provider and model.
- Isolated root: `/var/lib/pip-planner-probe.QPfDAa`.
- Board: `pip-isolated-planner-probe`; task: `t_6960008a`.
- Session: `20260905_083128_12a0bd`.
- Synthetic case: `repo:17#1@3`, `pip-fixture/local-only`.
- Immutable local fixture HEAD: `8417fda362de6e99fc3050edc1367bafde01b4ea`.

Rust prepared its own schema-8 ledger, managed profiles, and exactly one
ordinary parentless task. A transient systemd service ran the real Hermes CLI
dispatcher once, without a spawn callback. The worker ran as `pip-control`
with a read-only system, hidden operator homes, no privilege escalation, and
production state/credentials/control paths hidden. Only the isolated Hermes
home and fixture workspace were writable. The unit had a 16-minute cap and
control-group cleanup. Production services were never started.

The planner inspected the real fixture source and supplied issue, verified the
immutable bundle digest and its one event-payload digest using a local script,
and wrote Markdown and JSON plan artifacts. It diagnosed `split('\n').count()`
versus the issue's required `str::lines()` semantics. It did not modify tracked
source, fetch a repository, implement the fix, or publish anything.

## Real integration defect and regression-first fix

The first Rust verification failed with:

```text
MalformedResult("data did not match any variant of untagged enum WorkerResult")
```

The worker's JSON artifact was a valid strict contract. Stock Hermes had added
`worker_session_id` and `artifacts` to the durable run metadata. Upstream
`tools/kanban_tools.py` stamps the session and merges completion attachments;
`hermes_cli/kanban_db.py` can additionally add `_staged_artifacts`.

A new result-cycle test reproduced the same failure before implementation.
The Pip adapter now validates and removes only these three documented transport
annotations from a cloned contract view. Original metadata is preserved. No
upstream code or contract schema was loosened. Tests prove malformed annotation
shapes, unknown fields (including unknown underscore-prefixed fields), and
task/model drift remain rejected without recording a run.

The same completed task was then read again by the corrected Rust code. No
second provider run, manual completion, result rewrite, or metadata repair was
performed. Its original metadata SHA-256 remained:

`7aa832125f07363bb73db739d926d09b24048ac033cc088d55a6f88177135b0e`

Validation also passed for the full default Rust workspace suite, Clippy with
warnings denied, formatting, and shell syntax. The focused suites contain ten
Hermes read/transport tests and five result-ingestion tests. Real-provider
execution remains explicitly ignored in default CI and must be opted into.

The opt-in Rust test printed:

```text
ISOLATED_REAL_PLANNER_CONTRACT_ACCEPTED t_6960008a
test result: ok. 1 passed; 0 failed
```

Acceptance checked the task identity, exact fixture base, plan artifact
existence/confinement/content, and equality of the artifact JSON with the
extracted contract. Production result ingestion recorded one run and one
pending `PUBLISH_PLAN` effect. A second ingestion was idle. The isolated case
remained `PLANNING`, since publication was intentionally not executed.

## Limits and follow-up findings

- This is an offline fixture with the contract field guide explicitly supplied
  in its checkout. Before a live canary, normal workers must have a reliable
  packaged path to this guide, not a reference to a nonexistent file in MDK.
- The sandbox did not expose `rustc`. The planner reported that accurately and
  did not claim Rust tests ran. Some inline commands were blocked by Hermes's
  command scanner; file-based evidence verification succeeded without disabling
  the scanner. Toolchain availability remains a runtime packaging check.
- The wrapper stopped its cgroup after durable `done`; Hermes's final
  conversational response was interrupted. The claim is about the completed
  durable run and its matching artifacts, not final-response delivery.
- Running stock Hermes seeded bundled skills into the managed profile.
  A subsequent isolated `bootstrap-runtime` preflight correctly refused
  `profiles/planner/skills/.bundled_manifest` as unmanaged. Pip's bootstrap
  ownership rules need a stock-Hermes-compatible solution before deployment;
  do not delete arbitrary skills or patch upstream to bypass this.
- This does not prove builder execution, review fan-out, remediation, final
  review, CI disposition, or the live GitHub evidence path.

## Production preservation and cleanup

The production policy still hashes to
`9750f3bd6d3d3cd0216ae0ab19858719de60f371d694899e49169a1231c34196`.
The production schema-7 ledger still contains the same abandoned MDK #891
case, two events, two evidence records, zero runs/projections, no pending
outbox effects, and 35 webhook deliveries. The gateway and all three execution
timers remain inactive; upstream Hermes has no worktree changes.

The isolated root retains nonsecret ledger, task/run, plan, and log evidence.
The two temporary provider-auth copies and disposable Rust build cache are
removed after worker shutdown. The real service authentication is untouched.

See [the probe runbook](../runbooks/isolated-planner-probe.md) and
`crates/pip-control/tests/real_planner_probe.rs` for the repeatable boundary.
