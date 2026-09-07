# Signed canary recovery — 2026-09-07

This checkpoint proves signed publication, not final readiness or permission to
merge. Broader cleanup is deferred while the existing canary finishes, per JG's
latest direction.

## Release and preserved state

- Installed source: `5d4fdb2c3a9d14e84d3c5444c31bdfaaa5750397`.
- CI `34133415495` and signed deployment `34133415508` passed.
- Release manifest: `673b9e8d66bcba2de6d853f1a25c132676ecf34672da975a523c6dc7de681c8e`.
- Binary: `91ddc6a1853b059537382866f02df18d304e9233228b313756fee0bd96ae8629`.
- All 26 artifacts verified locally, through Pirate's preinstalled verifier,
  and from the actual installed release directory using the permanent trust key.
- CI exercised optional controller credentials, two-UID workspace handoff,
  signing credentials/sandbox, root recovery and installation lifecycle checks.
- Fresh quiescent backup: `/var/backups/pip-upgrade-5d4fdb2.0TJ0mz`, root-only.
- Before/after-install SQLite dump SHA-256:
  `97438844276d1e56354d2d6f1ac347054256e716579663e37d9c017a565b684e`.
  Integrity and foreign-key checks passed. Schema remains 10.
- Active policy bytes were preserved. Their SHA-256 remains
  `7125de44f82b4ecd6454198aff44ad9a2f426755678235cd1961d77fdbca23ec`.

The new controller unit uses optional credential-store lookups. The original
root-only credential sources and worker isolation remain unchanged. Its live
controller successfully performed authenticated signed publication. This is
not a live missing-App-key outage test.

## Accepted build, registered key and publication

JG registered signing-key ID `1161893` on `agent-p1p`. The API's public key exactly
matches Pirate's approved public key, fingerprint
`SHA256:uoZahdy4QImrKeSOfbsfX6FeweFeHC+eEJ2OXVlO7wg`. The private key stayed on
Pirate and was never sent to a worker.

With execution stopped/disabled and the direct queue empty, the ordinary
`authorize-publication-retry` command accepted request
`operator-sign-publication-993-20260907` against case `repo:1055628515#993@3`,
revision 26 and unsigned head `625bb4299187139461a64fd6eb5337ea35804261`.
Only the operational pause flags changed temporarily; the exact active policy
was restored before reconciliation. The authorization appends history and
grants zero model attempts.

The controller published signed head `53ac3d8f8143ea9f186bf677f59c3a16f2632fca`
on the original planned base `897111a9d9a9772b824cb4ed0ff60b9cb1242f5f`.
GitHub reports author `agent-p1p`, signature `verified: true`, reason `valid`.
Both unsigned and signed commits have tree
`21fbde518c149a74d43e3e92085194a568f16c34`, verified in the retained workspace.
The initial post-push PR reconciliation rejected an identity observation;
a subsequent normal reconciliation accepted the same signed head and updated
PR #1726. No identity guard was bypassed and no ledger row was manually edited.

The PR description now uses human-readable checks and finding summaries, with
structured build evidence retained in Pip. The prior JSON blocks are no longer
in its controller-written description.

At this checkpoint the case is `WAITING_CI`, revision 28, same plan/remediation
round, with six completed and ten failed direct attempts unchanged. Fresh CI
run `34134517302` is running. Old-head approvals do not count for the signed
replacement. Required re-reviews, holistic final review and human-held readiness
remain to be proven. The PR is still draft and no merge occurred.

The dedicated gateway and three execution timers were resumed. Changed trigger
timestamps and finite next firings were verified for all three timers. All observed
oneshot results are successful. Conversational Hermes stayed active and was
not reconfigured. Do not restore the pre-recovery ledger backup over new work.

## Review recovery after signed-head CI

The signed head's CI run `34134517302` passed. Both fresh required reviewers
produced approvals with no blocking findings, but those outputs did not complete
the review stage: direct attempt 17 failed while parsing Cursor's transcript,
and the failure bound escalated the case before the native review was accepted.
The direct result file independently passed contract validation. Ordinary Rust
interpolation braces in preceding progress prose caused the transcript failure.

- `1d70f5c` fixes that scanner without accepting malformed or competing contracts.
- `7700a21` makes audited retry validation resolve current frozen dispatch
  references through the existing digest/case-checked resolver. Its regression
  first reproduced the live rejection; tests retain wrong-role, observer,
  deadline, root, stale-request and unchanged-history checks.
- Installed source: `7700a21c47368e270bb736f9efc7512e44f87fcd`.
- CI `34138052044` and signed deployment `34138051885` passed, including the
  disposable Linux lifecycle gate. All 26 installed artifacts verified.
- Manifest: `53f72e545f4c6db4c147e4add8b4de35a6e8ec809b5b562bfbd7230b81557109`.
- Binary: `043d3cc6b9fa26e1f6972fbb2d47744fef40cd622e7c62c4574c482405329fac`.
- Root-only backup: `/var/backups/pip-upgrade-1d70f5c.RSlDas`.
- Both installations preserved the logical ledger dump hash
  `5f6053c91839c018bf92dbc60e3ed3d6f977b96558859598bf656c5699398fbb`.

With execution stopped and policy inert, the existing `authorize-review-retry`
accepted request `operator-review-retry-993-parser-20260907` at revision 30,
with seven case-specific failed attempts. It appended one bounded retry grant;
no old result, attempt, plan, PR head or deadline was rewritten. The exact active
policy was restored before resuming the dedicated runtime.

The controller revalidated current-head CI and moved the case to `REVIEWING`,
revision 32. Fresh native task `t_e5efc1b2` and direct attempt 18 both completed
with accepted approvals on the signed head. The controller published human-readable
GitHub reviews `5133675338` (general) and `5133675731` (security/performance),
without visible JSON evidence blocks. Structured evidence remains in Pip.

The case reached `FINAL_REVIEW`, revision 35; fresh live preflight was accepted
at revision 36. Final native task `t_71f141df` returned `READY` on the signed head,
which the controller accepted as `SHADOW_READY`, revision 37. Optional comparison
attempt 19 also completed with an accepted advisory approval on that same head.

The controller published its [human-held readiness notification](https://github.com/marmot-protocol/mdk/pull/1726#issuecomment-5573042895).
GitHub confirms the exact signed head, valid signature, open/draft PR, clean
mergeability and no merge. No case effects remain pending; all three execution
oneshots report success. SQLite integrity/foreign-key checks pass. The end-to-end
canary is now proven through human-held readiness, not autonomous merge or full
lean-architecture completion.

At handoff all dedicated execution units remain enabled, with repeated timer
firings and finite next runs. The separate conversational gateway is still active.
Both temporary sudoers files (`99-pip-temporary` and the expired
`99-pip-temporary-admin`) were moved to the root-only backup above, not destroyed.
`visudo -c` passes and a fresh noninteractive `sudo -n true` now requires a
password. Ordinary password-authenticated administration remains available.
