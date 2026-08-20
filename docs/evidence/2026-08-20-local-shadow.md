# Local non-dispatching shadow evidence — 2026-08-20

**Evidence boundary:** local Rust development binary plus live read-only GitHub
API. This is not a signed release, installed service, live Hermes run, provider
run, or production activation.

## Source and policy

- Runtime source commit: `68888597cd8911c11ed129550ac14175d95d3902`
- Repository policy: `config/target/repositories/mdk.json`, revision 1
- Repository identity: `marmot-protocol/mdk`, numeric ID `1055628515`
- Observation time input: `2026-08-20T12:00:00Z` (`1787227200`)
- Intake enabled: false
- Repository paused: true
- Dispatch enabled: false
- Merge mode: shadow
- Autonomous merge: false

The GitHub credential was obtained from the existing authenticated `gh`
keyring into a mode-`0600` temporary file, used only for two bounded reads, and
unlinked by an exit trap. No credential or token value entered the repository,
command output, report, or task body.

## Reconciliation result

Two consecutive reconciliations over the same observation time produced
byte-identical JSON with SHA-256
`4ab6d83ffcf9615dbc5714bafcf92d37e4fd019c7dae20fcffb969c86409a8e7`:

```json
{"candidates":[{"blockers":["INTAKE_DISABLED","REPOSITORY_PAUSED"],"decision":"INELIGIBLE","issue_id":5050325280,"issue_number":1240,"latest_label_actor_id":292420120}],"dispatch_enabled":false,"intake_enabled":false,"mutation_count":0,"observed_at":1787227200,"policy_revision":1,"report_format":1,"repository":"marmot-protocol/mdk","repository_id":1055628515}
```

The existing `pip-ok` label therefore discovers one issue through the generic
label path, but paused policy makes it ineligible. The live actor is represented
only by canonical numeric ID and is present in the configured trusted-actor
set. The reconciler reported zero mutations and has no task, comment, branch,
PR, review, notification, or merge write path.

## Hermes and provider observations

No `hermes` executable is available on this workstation. The local
`~/.hermes` directory contains only the `uv`/`uvx` launchers, one plugin, logs,
and a minimal config; it has no profiles, auth links, board database, or task
state that the Rust reader can ingest. Live Hermes parity is therefore
**unavailable**, not green.

The installed Cursor Agent reports version `2025.09.18-7ae6800`, is not logged
in, and its command surface does not advertise the required safe `models`
probe. The Rust provider boundary consequently fails closed before any worker
run and cannot verify either exact configured Cursor model. OpenAI-Codex roles
also cannot be live-probed through the absent Hermes runtime.

Offline outage/recovery tests prove that both adapters can recover only through
a fresh read/probe and never substitute a model or issue a Hermes write. Live
provider recovery and live Hermes evidence still require a compatible runtime
and authentication boundary; those are distinct from the successful GitHub
shadow evidence above.

## Mutation statement

This evidence collection changed no GitHub issue, label, comment, branch, PR,
review, Hermes board/task, provider session, systemd unit, installed release, or
control-plane ledger. Issue `1240` was already labeled before this run.
