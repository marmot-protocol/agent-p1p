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
