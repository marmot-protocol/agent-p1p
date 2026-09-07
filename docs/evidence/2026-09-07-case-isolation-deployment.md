# Case-isolation release deployment

Observed 2026-09-07, approximately 14:01 UTC. This is a dated checkpoint,
not completion of the lean migration or authorization to merge.

## Provenance and validation

- Source: `8ca4e7422e49269aa8c8720791d1ac5ccd76fb29`.
- CI: `34129502446`, success.
- Signed deployment build: `34129502489`, success.
- Manifest: `00e3b092ccbef83d546c2f98c96a9c371cf96a4c40fe4eea90b963412913ade4`.
- Binary: `381a7dcdb4033de42ac0a1da1a4ed04f39701c5097f6cd583f59ccbfcade8e4b`.
- All 26 artifacts verified locally and on Pirate with its existing verifier
  and permanent trust anchor before installation.
- Linux job `101765948000` passed the two-UID workspace handoff, controller
  signing credential sandbox, actual-root recovery checks, and install,
  reinstall, upgrade, rollback and restart recovery gates.
- Exact-source full Rust tests passed in CI. Final local direct-worker tests
  (21) and all-target/all-feature Clippy passed.

## Preserved live state

The private ledger-copy migration first verified schema 8 to 10 preservation.
The actual installation then compared every pre-existing table's ordered JSON
rows against a fresh stopped-state backup; all 14 tables matched, including
events, results, findings, frozen dispatch definitions, attempts and outbox.
Integrity and foreign-key checks passed. Schema metadata/constraints changed;
historical table data did not.

- Root-only stopped backup: `/var/backups/pip-upgrade-8ca4e74.YqzcJl` (0700).
- Ledger owner/mode: `pip-control:pip-control`, 0600.
- Active policy revision 7 preserved byte-for-byte; SHA-256
  `7125de44f82b4ecd6454198aff44ad9a2f426755678235cd1961d77fdbca23ec`.
- Root-only isolated probe copies remain at `/var/tmp/pip-migration-check.CAEi9J`.
  These are diagnostic copies, not rollback targets for later live work.

The dedicated upstream Hermes gateway returned exit 1 when sent SIGTERM.
The initial preparation stopped before installation. Journal evidence showed
the requested shutdown, MainPID 0 and no worker/controller processes. Installation
continued only from that verified quiescent state; no Hermes fork or service
exit-status weakening was introduced.

## Resumed runtime and remaining gate

Three managed profiles reconciled under the same active policy; no new board or
profiles were created. The dedicated gateway and three previously enabled timers
resumed. Repeated firings and finite next runs were verified. Controller format-2
observations at epochs 1788789669 and 1788789698 reported success, no new dispatch,
and pending final preflight for #993 with `PR_MERGE_STATE:blocked`.

The canary remains `FINAL_REVIEW`, revision 26, plan 1, remediation round 1;
PR #1726 remains open/draft at `625bb4299187139461a64fd6eb5337ea35804261`.
No new model attempt or publication was authorized. The approved signing key
still requires registration, followed by audited signed republication, fresh
head-bound CI/reviews, and holistic final review. Existing JSON GitHub comments
were not rewritten; the new human-readable format applies to new publications.

Conversational user `hermes-gateway.service` remained active and untouched.
Live peer-outage drills, new compact dispatch/provider execution and the other
requirements in `docs/completion-audit.md` remain separate incomplete gates.
