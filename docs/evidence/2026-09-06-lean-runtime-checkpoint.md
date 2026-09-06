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

Still outstanding: complete failure isolation and saved-job upgrade semantics,
compact worker evidence, normal operations simplification, live PR/review/CI
proof, and removal of legacy Python after that cutover proof.
