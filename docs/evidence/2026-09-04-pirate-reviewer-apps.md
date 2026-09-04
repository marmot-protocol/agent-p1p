# Pirate reviewer-App evidence, 2026-09-04

This note records an installed-key, read-only GitHub App verification. It
contains no private-key or token material and does not authorize workflow
activation or GitHub mutation.

## Installed boundary

The four installed credential files were root-owned regular files with mode
`0600`. The non-secret metadata bound distinct Apps and installations to MDK's
numeric repository identity:

```text
reviewer-general: app 4803765, installation 158461187, repository 1055628515
reviewer-secperf: app 4803783, installation 158461566, repository 1055628515
```

## Token and read probes

For each role, a short-lived RS256 App JWT was signed using its installed PEM
and exchanged for an installation token. The token was retained only in the
probe process's memory: it was not printed, persisted, or passed through argv
or the environment. The probe then performed authenticated, read-only requests
for repository 1055628515, MDK issue 1652, and MDK pull request 1667.

Sanitized results:

```json
{"app_id":4803765,"expires_at":"2026-09-04T09:25:20Z","installation_id":158461187,"issue_number":1652,"issue_status":200,"mint_status":201,"permissions":{"issues":"read","metadata":"read","pull_requests":"write"},"pull_number":1667,"pull_status":200,"repository":"marmot-protocol/mdk","repository_id":1055628515,"repository_selection":"all","repository_status":200,"role":"general"}
{"app_id":4803783,"expires_at":"2026-09-04T09:25:22Z","installation_id":158461566,"issue_number":1652,"issue_status":200,"mint_status":201,"permissions":{"issues":"read","metadata":"read","pull_requests":"write"},"pull_number":1667,"pull_status":200,"repository":"marmot-protocol/mdk","repository_id":1055628515,"repository_selection":"all","repository_status":200,"role":"secperf"}
```

Both Apps therefore have the intended live grant: read issues and repository
metadata, and read/write pull requests, across the organization's selected
`all` repository scope. No review was posted during this probe.

## Installed policy and branch rule

Installed policy revision 2 binds the same reviewer actor IDs and the exact
`Required CI` context. A live API reread found active MDK ruleset `Safe Master`
(ID 13122882), targeting the default branch, with required status check
`Required CI` sourced from GitHub Actions integration ID 15368.

The live rule also reported `strict_required_status_checks_policy:false`, zero
required approvals, and unresolved-thread enforcement off. Pip's own final
preflight still requires its two exact-head role approvals and resolved review
threads before it can declare a case shadow-ready. The ruleset currently has
always-on bypasses for organization administrators and repository role 5; that
is an external repository-governance decision, not authority granted to Pip.
