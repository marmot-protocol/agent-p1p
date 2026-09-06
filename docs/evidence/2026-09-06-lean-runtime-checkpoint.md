# Lean runtime checkpoint

This is dated evidence, not an end-to-end completion claim.

## Deployment and retained state

Protected CI run `34059095699` passed Rust, systemd lifecycle, and build/sign
for source `b2a14f63be4de54d0922d2bab8bc05f0d0d782a8`.

- Manifest: `1379e1a6928ae9185ef260097890ff7b02d265946d925f5168c40909993cf758`
- Binary: `6a279fa690ea220b5e59d21ce216905893ca5b3d3fb296902636a76e529a3596`
- Installer: `a6a3127d7541f0a339caa253cd7afe79b7f99b6edbe3f78e155a80d60cd9e8a8`

The cohort signature and all 26 artifacts were verified locally and on Pirate.
Installation preserved the paused policy and byte-identical ledger. Both
execution services report `MemoryDenyWriteExecute=no`; controller and ingress
remain restricted. Hermes source and the conversational gateway were untouched.

Normal authorization reconciliation subsequently observed removal of #1228's
label and appended its abandonment at state revision 6. No attempts, accepted
plan or prior evidence were reset. The old #891 synthetic gate card was archived
through Hermes. Normal retention retired the clean #1639 case workspace with its
branch preservation checks.

## Canary selection and actual boundary

#1260 was investigated but not authorized: the backend still emits
`GroupInfo.subject: None`, a dependency shared with #1228. No symptom-only fix
was attempted. The open-issue scan found the shared Hermes/OpenClaw subject
issues; those need coordinated scope rather than a display-only canary.

#993 remains open with no linked/open fix PR. Current CLI message rendering
interpolates untrusted plaintext without the existing TUI sanitizer. The local
output-boundary change is suitable for planner validation and regression-first
implementation. Its `pip-ok` event created `repo:1055628515#993@3`, policy 7,
state revision 1, through normal intake. It has no PR or completed worker yet.

Workspace allocation then failed with Git status 128. A shell with the service
UID succeeded, but a credential-free transient service with the controller's
sandbox reproduced:

> fatal: Could not make .../.git/refs writable by group

`git init --shared=group` sets setgid, which `RestrictSUIDSGID=yes` forbids.
The same unnecessary bit existed in subsequent artifact creation. The proposed
fix removes those bits, not the sandbox restriction: both service accounts
already have the same primary group. The lifecycle fixture now runs preparation
under the controller unit restrictions as well as editing under the worker unit.

The stricter mode tests reproduced the unnecessary bits before the fix. The
full Rust suite then passed 328 tests with seven explicitly ignored integration
tests, and all-feature/all-target Clippy passed. The expanded Linux lifecycle
passed controller preparation with `RestrictSUIDSGID=yes`, the two-UID handoff,
both execution JIT boundaries, clean install/reinstall/upgrade, intentional
rollback and restart recovery. These are service-boundary proofs, not a claim
that a model has yet completed #993.

Execution was stopped without removing #993's authorization or resetting its
pending dispatch. Webhook intake remains active. Resume the same case after the
verified repair; do not create another case to avoid its history.

## Scope reduction

The first pass removed the alternate in-process direct executor, recovers saved
dispatch definitions, isolates detached-review failure budgets, delays reviewer
credential acquisition until publication, and makes cleanup failure nonblocking
when capacity is still healthy. Confirmed non-starts have durable exponential
backoff with atomic evidence and replay/restart tests.

Subsequent local commits use Hermes's supported JSON configuration interface and
remove the unused autonomous merge coordinator/API (over 900 lines including its
obsolete execution tests). Human-held disposition and exact-head final-preflight
tests remain. Historical core/ledger representations are retained for reading
old state, not exposed as an autonomous execution capability.

## Repair deployment and resumed planner

CI run `34060285413` passed every gate for
`cd738eeb15432310c83d0b60efc62287cd013113`. The verified cohort has manifest
`768c98927edcecdc099eb1fae73f593ffa4d647b337dae92c8efb6438c7a1712` and binary
`346436f19611cb57b05132438edd2912b4f70d9672ba16db8b41dffdc157d989`.
Installation on Pirate preserved the ledger byte-for-byte. After restoring the
same approved revision-7 policy, workspace preparation and dispatch succeeded.
Both the #993 workspace and its private `.git` directory are `pip-control` owned,
mode `0770`, with the setgid restriction still enabled. Hermes reports planner
task `t_bf698ea6` running, started at Unix `1788729612`.

No case reset, renewed label cycle, extra planner or provider substitution was
used. Controller, direct-worker, consumer timers and the dedicated dispatcher
are active again. This establishes live planning startup, not a completed plan
or a PR.

The subsequent local installer fix preserves validated operator configuration
instead of resetting it to seed policy on every upgrade. It passed 329 Rust
tests and Clippy; the Linux lifecycle retained exact active-policy bytes through
reinstall, upgrade, injected rollback and reboot while all execution remained
disabled. It has not yet been deployed.

Still outstanding: complete failure isolation and saved-job upgrade semantics,
compact worker evidence, normal operations simplification, live PR/review/CI
proof, and removal of legacy Python after that cutover proof.
