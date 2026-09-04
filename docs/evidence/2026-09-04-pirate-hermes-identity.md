# Pirate conversational Hermes identity migration evidence

Date: 2026-09-04

This record covers the user-facing Pip Hermes gateway and Marmot identity. It
does not activate the Rust control-plane gateway, controller, direct worker, or
shadow reconciler.

## Boundary

Pip now has two deliberately separate Hermes roots on Pirate:

- `/var/lib/pip/hermes` is the `pip-control`-owned execution runtime. Its
  managed profiles disable memory and messaging, and its Kanban board is only
  an execution projection of the Rust ledger.
- `/home/jeff/.hermes` is the user-facing conversational gateway. It carries
  Pip's persona, operator preferences, provider login, and Marmot platform
  configuration, but it is not a scheduler or workflow authority.

No legacy Kanban board, task, run, session, workspace, state database, cron
job, profile directory, cache, log, backup, LSP install, or bundled Hermes
checkout was copied from Vault.

## Vault quiescence

Nine legacy Pip cron jobs were paused before the identity cutover:

- `pip-issue-scanner`
- `pip-stale-pr-sweeper`
- `pip-worktree-janitor`
- `pip-pr-budget-guard`
- `pip-worker-watchdog`
- `pip-pr-comment-watcher`
- `pip-ngit-pr-comment-watcher`
- `pip-ngit-issue-scanner`
- `pip-daily-activity-summary`

Hermes then reported zero scheduled jobs. The Vault user units
`hermes-gateway.service`, `wn-agent-hermes.service`, and
`hermes-dashboard.service` were stopped and disabled. The old source trees
remain on Vault for rollback until an end-to-end inbound and outbound Marmot
message succeeds on Pirate.

## Identity transfer

The stopped `/home/jeff/.marmot-agents/hermes` tree was copied with owner-only
permissions, excluding only the runtime lock and Unix socket. The source and
target regular-file trees matched the aggregate SHA-256 digest
`9eafa62bb70e2e714bee4c47d23aab31d98fe91a9f3a3164b1b95655be18ccda`
before the target was started.

The reused identity reported `created: false` and retained public identity:

```text
npub1pnsqysgwscmkxmetu8zgdg0cyz8ej9329ds2y2mcqym67wuvyg7schtgas
```

The exact Vault connector release was installed fresh on Pirate:

- `wn-agent` version: `0.9.15`
- binary SHA-256:
  `eaaa781843de7deaac5d384b13bb4ca1036600a1cf54eb0cb81632e6222d9961`
- plugin release: `wn-agent-v0.9.15`
- plugin source commit: `f7159003eb76e00764c0320ba783a0f6dde7486f`

The release installer successfully installed and started the connector but its
legacy YAML patcher rejected Hermes 0.21's sequence indentation. The config was
therefore written through Hermes 0.21's own `hermes config set` interface and
passed `hermes config check`. The connector reused the copied identity and its
Unix socket is owner-only.

Only the OpenAI Codex provider entries were migrated from the old user auth
file. Other provider credential pools and the old OpenRouter API key were not
copied. Only `MARMOT_*` environment entries were migrated; Telegram and its bot
token were deliberately not copied.

The old persona and memory documents were curated during migration. Stale
Vault host references, Pip v2 naming, old auto-merge policy, legacy model lanes,
and multi-repository scheduler state were removed. The new documents state that
the Rust ledger is authoritative and that the conversational gateway must not
recreate or bypass control-plane automation.

## Live result

On Pirate:

- `wn-agent-hermes.service` is enabled and active;
- user `hermes-gateway.service` is enabled and active;
- Hermes reports Marmot configured through plugin `0.9.15`;
- Hermes reports the OpenAI Codex provider authenticated;
- an exact `gpt-5.6-sol` local provider probe returned only
  `PIP_PROVIDER_OK`;
- an outbound message to the existing Marmot home channel succeeded as message
  `8b5edcbd58edbb020f2084571de91ddd06b02c2afa82d9a67abcf68104a8fa9f`;
- the Rust `pip-hermes-gateway.service` and every execution timer remain
  disabled and inactive; and
- the release-build cache is empty of `pip-release-build-*` directories and
  `pip-*.bundle` files.

The remaining acceptance check is a reply from the approved White Noise account
that receives a model-generated response from the Pirate gateway. Do not delete
the preserved Vault Hermes or Marmot data until that succeeds.
