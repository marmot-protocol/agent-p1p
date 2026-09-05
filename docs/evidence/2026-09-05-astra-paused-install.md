# Astra canary-readiness release: paused installation

JG approved the protected release environment for
[run 33989461746](https://github.com/marmot-protocol/agent-p1p/actions/runs/33989461746).
All three jobs passed: Rust verification, disposable-systemd lifecycle, and
build/sign. The release was installed on Pirate without activating execution.

## Exact deployment

- Source: `6b0a1444b9b38c85a5acc5a7d618a9dfe04629f1`.
- Manifest/release: `fa0a188a8d6f838063d976dcc3f573e33b3470865a379b1cf3d4ac0efc8440fd`.
- Binary: `f4154b0b2d03fe7630b340ec699a1ba7f0ecdea13a01f3a4c646d1038a1f28cc`.
- Installed inert policy revision 6:
  `f6868eafe0262208e3afa6fe17d566972ba8250342b7f2dfb2aab296a81ffe7f`.
- Permanent public-key file SHA-256 remains
  `f7a0f0eca7c35e29b58d2c07212524f3bacd64f2c3a80ae3f2b8bab466ecd34b`.

Both the local trusted verifier and Pirate's previously installed binary verified
the signature and all 26 artifact digests. Outer checksums passed; the copied
installer matched the signed resource and reviewed source byte-for-byte. The
root installer reverified the pinned manifest and binary from private staging.

Host staging is
`/home/jeff/.cache/pip-deployments/6b0a1444b9b38c85a5acc5a7d618a9dfe04629f1/run-33989461746/`.
It retains the cohort, verification output, operator script and installation log.
Its root-only `pre-install/` holds online backups of ledger and Kanban databases,
the previous policy/installer and managed profile configuration/skill links.
No auth file was copied into this evidence archive or repository.

The first operator-script invocation stopped before mutation because `jq` is
absent. Structured validation was changed to use existing Python; no package was
installed. The second invocation completed with
`PIP_6B0A144_PAUSED_INSTALL_AND_BOOTSTRAP_OK`.

## Post-install verification

- Installer completed and all three managed profiles reconciled to revision 6.
- Effective provider/model: `openai-codex/gpt-6-astra`; reasoning is planner
  `xhigh`, general reviewer `high`, final reviewer `xhigh`. Fallbacks are empty;
  stock `lsp.enabled` is false. Direct-provider models are unchanged.
- Intake remains disabled/paused; dispatch disabled; merge shadow/non-autonomous.
- Execution timers and gateway remain disabled, with no worker MainPIDs.
  The gateway's preserved failed state is not a running process.
- Ledger and Kanban logical dump hashes are identical before/after installation
  and bootstrap. Auth file hash is unchanged. Ledger integrity and foreign-key
  checks pass. Schema remains 8; both historical cases and the frozen Sol task
  are preserved, with no accepted plan, new run, or PR.
- Installed gateway has only Hermes state and the private RAID-backed scratch
  subtree writable; repository/worktree source remains read-only and
  `MemoryDenyWriteExecute=yes` remains enabled.
- Conversational user Hermes and webhook ingress remain active. Stock Hermes
  code remains unmodified.

The offline sandbox fixture from the exact release source was rerun after
comparing its service template byte-for-byte with the installed unit. Invoke the
fixture script with `bash` (its source file is not executable). It returned
`PIP_OFFLINE_PLANNER_SANDBOX_OK`, exit 0, 1.999 seconds, 169.3 MiB peak memory.
Report: `/var/lib/pip/worktrees/hermes-scratch/offline-bslr1h3k/results/report.json`.
Its actual stock-Hermes CLI result passed the Rust exactly-once ingestion test.
This was an isolated, network-denied fixture with zero provider calls, not an
Astra inference or live-case test.

## Next authorization boundary

Live GitHub still shows only #1639 with `pip-ok`. Its case remains PLANNING and
the interrupted task retains its original Sol binding. Do not restart that task
under the new profile or reuse the old activation script/rollback revision.

Before another live canary, obtain explicit disposition of #1639, revoke its
authorization through the normal audited path, and select a separately
authorized fresh issue. The previous provider `cyber_policy` rejection remains
unresolved: this deployment does not authorize replaying it through another
model or establish that it was a false positive. No labels, tasks, historical
rows, or authorization events were changed during installation.
