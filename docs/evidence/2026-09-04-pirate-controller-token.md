# Pirate controller credential boundary — 2026-09-04

This evidence records a live, non-dispatching verification of the Pip
controller GitHub credential on Pirate. It does not authorize intake,
dispatch, worker execution, publication, or merge.

## Installed release

- Release ID:
  `bcc62a0c8a317882848afe86e23089d1cf7d5e3bf2e283dc7745c8aa2655087d`
- Source commit: `be73ad9d72702a4b7cecf11ff433bdf8f5bd8603`
- Credential path: `/etc/pip/github.token`
- Credential metadata: root-owned, group `root`, mode `0600`, regular file,
  94 bytes

No credential contents were printed, copied into this repository, or placed in
a process argument or environment variable during verification.

## GitHub identity and read boundary

The staged credential was read directly by a short-lived verifier on Pirate.
GitHub returned:

- authenticated login `agent-p1p`;
- authenticated actor ID `292420120`;
- repository `marmot-protocol/mdk`;
- repository ID `1055628515`;
- repository permissions reporting pull, triage, and push access without admin
  or maintain access;
- HTTP 200 for the selected issue, issue events, open pull requests, exact-head
  check runs, and exact-head combined commit status endpoints; and
- accepted-permission headers of `issues=read`, `pull_requests=read`,
  `checks=read`, and `statuses=read` on the corresponding probes.

The read-only probe did not create or mutate any issue, label, comment, branch,
pull request, review, check, status, workflow, hook, or repository setting.
Configured write permissions remain to be exercised only through the explicitly
authorized shadow canary.

## Installed service probe

After promotion to the root-owned production path, one manual start of
`pip-webhook-consumer@mdk.service` loaded both systemd credentials and ran
against the empty production spool. The command completed successfully with:

```json
{"delivery_id":null,"intake":null,"report_format":1,"result":"EMPTY"}
```

The consumer deactivated successfully. A subsequent read-only host inspection
found zero receipt, pending, and processed entries.

## Preserved inert state

- `pip-webhook-ingress.service`: enabled and active
- `pip-webhook-consumer@mdk.timer`: disabled and inactive
- `pip-controller@mdk.timer`: disabled and inactive
- `pip-direct-worker@mdk.timer`: disabled and inactive
- `pip-hermes-gateway.service`: disabled and inactive
- installed policy: intake disabled and paused, dispatch disabled, shadow merge,
  autonomous merge disabled
- production policy GitHub actor fields: still unset
- production required CI contexts: still empty

The next credential-bound ingress gate is one controlled pending delivery, not
timer activation. Actor IDs, actual required MDK CI contexts, live provider
capability/recovery evidence, and explicit canary authorization remain separate
gates.
