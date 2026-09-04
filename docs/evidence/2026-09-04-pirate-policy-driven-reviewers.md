# Pirate policy-driven reviewer release evidence

Date: 2026-09-04

This record covers the inert installation of workflow version 3 and its
policy-defined reviewer instances. It does not authorize intake, dispatch,
worker execution, GitHub mutation, or merge.

## Exact release

- Source commit:
  `e4cbd333ede4197c31349b9e7259ee670311ed1e`
- Release ID and manifest SHA-256:
  `99099430157256bfb922226afe698c2a650fdeb5e3bcff0699e29f40ac68a78b`
- Binary SHA-256:
  `74544b67f3bcfd83656d104cbeacafdb8d89cabc76fe7882bec8752c6d57c306`
- Installer SHA-256:
  `8e54ec3f98a39fec01e85805b32b0738ab1295986058a8c677a3f54bf56c6eeb`
- Installed path:
  `/opt/pip/releases/99099430157256bfb922226afe698c2a650fdeb5e3bcff0699e29f40ac68a78b`

The cohort was built on Pirate from a verified complete Git bundle of the
exact source commit. A one-time Ed25519 key signed the cohort and was shredded
after independent verification with the previously installed trusted binary.
The canonical root-owned public key was replaced only after installation
succeeded. The build checkout and transfer bundle were then removed.

## Upgrade and regression gate

The first installation attempt failed closed before mutation because the new
binary required schema 7 while the installed ledger was schema 6. The release
installer was trying to open the old ledger through the strict current-schema
reader before taking its rollback snapshot.

The defect was reproduced in a new installation-lifecycle test. The installer
now snapshots any supported nonzero schema through a raw read-only SQLite
connection before opening the ledger for migration. Workspace tests, strict
Clippy, formatting, supply-chain checks, and the disposable systemd lifecycle
passed before the corrected commit was built.

The corrected live upgrade migrated schema 6 to schema 7 while preserving all
eight prior webhook-delivery records. The post-install ledger contained no
cases, runs, findings, outbox work, task projections, or direct attempts.

## Installed policy and runtime

Installed MDK policy revision 3 selects workflow version 3 with:

- required general reviewer `general-sol` using `gpt-5.6-sol` through Hermes;
- required security/performance reviewer `secperf-kimi` using `kimi-k3-max`
  through Cursor;
- detached shadow security/performance reviewer `secperf-opus` using
  `claude-opus-5-thinking-high` through Cursor; and
- builder `cursor-grok-4.6-high-fast` through Cursor.

The service-owned Hermes runtime retained the existing `pip-mdk` board and
reconciled its three Hermes-native profiles against the exact installed skills
and policy. Hermes reported version 0.21.0. This operation did not start the
Pip Hermes gateway.

Cursor Agent `2026.09.02-c22c1a3` advertised all three configured direct
models. Separate read-only `ask` invocations under `pip-worker`, with sandboxing
enabled in a temporary empty workspace, returned only `PIP_PROVIDER_OK` for
Grok, Kimi, and Opus. This proves current model availability and provider
authentication, not task-contract execution.

## Provider outage and recovery

A process-scoped outage drill directed an exact `kimi-k3-max` request to the
unreachable loopback endpoint `127.0.0.1:9`. It failed nonzero immediately with
`ECONNREFUSED`; a 45-second outer timeout bounded any unexpected provider retry.
A new request through the normal configured endpoint then returned only
`PIP_PROVIDER_RECOVERED` in under five seconds.

The drill did not alter global networking, Cursor authentication, production
policy, the direct queue, or systemd state. The production ledger SHA-256 was
identical before and after the failure and recovery. Its temporary empty
workspace and diagnostic output were removed. Offline controller tests remain
the evidence that a failed direct attempt releases its durable effect and that
the operational failure counter eventually escalates rather than retrying
without bound.

## Pip gateway supervision

The `pip-mdk` board was empty before the gateway check. The disabled
`pip-hermes-gateway.service` was started manually without enabling it. After
the launcher completed its exec transition, systemd reported one active main
process with:

- UID and EUID 997, the installed `pip-control` identity;
- cgroup `/system.slice/pip-hermes-gateway.service`;
- command `hermes gateway run --external-supervisor`;
- `HOME=/var/lib/pip/hermes/home`; and
- both `HERMES_HOME` and `HERMES_KANBAN_HOME` set to
  `/var/lib/pip/hermes`.

The board remained empty while the gateway was running. The service was then
stopped and returned to disabled/inactive; the board was still empty. Pirate's
separate user-facing Hermes gateway and WN Agent remained active throughout.

## Non-dispatching live reconciliation

A manual run of `pip-shadow-reconcile.service` used the installed controller
credential and policy to reread GitHub. It completed successfully with:

```json
{"candidates":[],"dispatch_enabled":false,"intake_enabled":false,"mutation_count":0,"policy_revision":3,"report_format":1,"repository":"marmot-protocol/mdk","repository_id":1055628515}
```

The exact observation timestamp is retained in the system journal and omitted
above because it is not a policy input.

## Preserved inert state

- `pip-webhook-ingress.service`: enabled and active
- `pip-webhook-consumer@mdk.timer`: disabled and inactive
- `pip-shadow-reconcile.timer`: disabled and inactive
- `pip-controller@mdk.timer`: disabled and inactive
- `pip-direct-worker@mdk.timer`: disabled and inactive
- `pip-hermes-gateway.service`: disabled and inactive
- failed system units: zero
- policy intake: disabled and paused
- policy dispatch: disabled
- merge mode: shadow
- autonomous merge: disabled

No issue, task, branch, comment, pull request, review, or merge was created by
this installation or its probes. Protected release-workflow evidence and
explicit single-issue canary authorization remain separate gates.
