# Remaining migration

The canonical target is [Pip architecture](pip-architecture-plan.md).
[Implementation status](implementation-status.md) records what is implemented,
installed and still unproven. Do not use the former numbered-phase roadmap or
historical activation scripts as the current operating procedure.

## Finish the real pipeline

Run one authorized issue through an accepted plan, local build, draft PR,
exact-head CI, every required independent review, bounded remediation when
needed, and final human-held readiness. Preserve the same case and its failures
while repairing infrastructure. A successful adapter probe or planner is not
cutover proof. A person reviews and merges; this project has no merge executor.

## Finish the lean boundaries

- Recover saved assignments across upgrades without silently changing inputs.
- Isolate job/capability failures; collect safe results during a dispatch pause.
- Keep confirmed non-starts out of work-failure budgets and back them off.
- Keep optional review failures and latency off the required workflow.
- Give roles compact evidence with retained immutable history.
- Replace one-off SQL and case-specific recovery scripts with supported operations.
- Keep working webhook intake, polling recovery and credential isolation.
- Use stock Hermes and a narrow Cursor adapter, with real compatibility tests.

The status document tracks the remaining gaps. Local tests, Linux lifecycle
tests, installed behavior and live provider/GitHub evidence are separate gates.

## Retire the Python reference

The deployed Python plane has already been retired; it is not a rollback path.
After the Rust cutover proof, remove its package, tests, wheel installer and CI
dependencies. Keep only useful language-neutral parity fixtures and small
upstream-interface test helpers. Historical source remains recoverable in Git;
there is no reason to maintain a second runtime indefinitely.

## Expand deliberately

After the pilot's behavior and recovery are accepted, increase issue concurrency,
then add another repository/board through validated policy. Do not infer
authorization for another repository, a new model, broader credentials or
automatic merge from a successful pilot.
