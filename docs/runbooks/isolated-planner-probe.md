# Isolated real-planner probe

This is an operator-only provider test, not a production controller command.
It runs one ordinary assigned Hermes task through stock Hermes and then feeds
the worker's untouched durable run metadata into Rust result ingestion. It must
not activate production, publish a GitHub comment, or reuse a canary case.

## Preparation boundary

Build the ignored `pip-control` integration test `real_planner_probe` from the
candidate source. Stage it and the source in a fresh private directory owned by
the service identity. The directory must contain:

- `ISOLATED_PROBE`, copied from `tests/fixtures/planner-probe/ISOLATED_PROBE`;
- `source/`, containing the candidate source, canonical skills, contract guide,
  and fixture;
- `credential/auth.json`, a temporary mode-0600 copy of the service's provider
  authentication. Never copy this file into the repository or test report.

Set these explicit environment variables when invoking the compiled test:

- `PIP_PLANNER_PROBE_ROOT`: that fresh directory;
- `PIP_PLANNER_PROBE_PHASE`: `prepare`, then later `verify`;
- `PIP_TEST_HERMES`: absolute stock Hermes executable;
- `PIP_TEST_SKILLS_COMMIT`: exact candidate skills commit;
- a clean `PATH` and isolated `HOME`.

Invoke the test binary with
`--ignored --exact isolated_real_planner_contract --nocapture`.

Preparation refuses an existing policy or ledger. It creates an offline
repository with synthetic identity `pip-fixture/local-only`, its complete
issue fixture, a private schema-8 ledger and board, managed profiles, and one
ordinary planner task using the candidate's Rust dispatch function. It records
the fixture HEAD and actual assigned task ID. No GitHub read or write occurs.
The model, provider, and reasoning configuration come from the candidate role
policy; no model substitution is permitted.

## Real worker boundary

Run `source/tests/fixtures/planner-probe/run-worker.sh` in a dedicated transient
systemd service, not the production gateway. Supply isolated `HOME`,
`HERMES_HOME`, `HERMES_KANBAN_HOME`, and `PIP_PLANNER_PROBE_ROOT`.

The service must have a finite runtime, control-group cleanup, no privilege
escalation, a read-only system, hidden operator homes, and no capabilities.
Only its isolated Hermes home and fixture workspace may be writable. Hide the
production `/var/lib/pip`, `/etc/pip`, any existing control socket directory,
the probe ledger, and the extra credential staging directory. An absent socket
directory must be optional in `InaccessiblePaths` (systemd's `-` prefix), not a
reason to fail namespace setup. The profile authentication under the isolated
Hermes root is the only provider credential the worker needs.

The wrapper performs exactly one real CLI dispatch with `--max 1`, then waits
for completion. It does not inject a spawn callback, reschedule a failed worker,
complete the card, or repair the worker's metadata. Stop the whole unit before
verification or cleanup. Inspect the Hermes session's model/provider record as
well as the worker's self-description.

## Acceptance and cleanup

Run `verify` outside the worker sandbox. It preserves the original metadata,
checks the real task identity, fixture base SHA, and a nonempty plan artifact
confined to the workspace. Production Rust ingestion must accept the result
once and only once, recording a run and queuing—but not executing—plan
publication. Inspect both Markdown and JSON plans, source cleanliness, evidence
claims, and any blocked tools separately; contract acceptance is not a claim
that every proposed test ran.

Preserve nonsecret task, run, plan, and verification evidence. After all probe
processes stop, remove both temporary provider-auth copies and the disposable
build cache. Verify production is still paused and upstream Hermes unchanged.

This test explicitly supplies the worker field guide in the fixture checkout.
It does not yet prove that a normal MDK checkout receives all required Pip
contract documentation, that the live GitHub evidence path works, or that the
builder/reviewer/final-review pipeline succeeds.
