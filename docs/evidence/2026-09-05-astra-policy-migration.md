# GPT-6 Astra policy migration

JG explicitly selected GPT-6 Astra on 2026-09-05. This records a source-policy
change, not deployment, inference success, or permission to replay the rejected
MDK #1639 request.

## New target

Target policy revision 6 selects `openai-codex/gpt-6-astra` for:

- planner: `xhigh`;
- general reviewer: `high`;
- final reviewer: `xhigh`.

The reviewer instance ID `general-sol` is intentionally retained as a stable
identity, not a model selector. Every result must carry its exact task-bound
model. Grok builder, required Kimi reviewer, and shadow Opus comparison are
unchanged. Intake/dispatch remain disabled; merge remains shadow/non-autonomous.

Canonical skills describe the new selection but require historical tasks to
retain their original bindings. Revision-4 Phase 9 activation, legacy role
manifests/configuration, frozen migration fixtures and prior evidence remain
unchanged. Do not use that old activation policy to activate revision 6.

## Availability and compatibility evidence

The official [Astra migration guide](https://developers.openai.com/api/docs/guides/latest-model)
supports retaining existing high/xhigh reasoning, requires Responses for tool
calling, and excludes temperature/top_p/logprob parameters. OpenAI Docs informed
this migration. Its local specialized migration-reference file was absent, so
the fetched official guide was used directly.

A fresh authenticated model-catalogue request through Pirate's worker OpenAI
credential returned 10 visible models including exact `gpt-6-astra`. Only the
boolean availability result and catalogue count were printed, not credentials.
Catalogue visibility is not a successful inference or proof of authorization
for the previously rejected workflow.

The installed Hermes OpenAI-Codex request path uses Responses. Inspection of
`agent/chat_completion_helpers.py` shows that path returns before the generic
temperature-building branch. Astra's full transport/context/compaction behavior
has not been certified; some installed model metadata explicitly covers older
GPT families. No Hermes fork or live model probe was performed.

## Validation and deployment boundary

The new exact-model policy regression failed against Sol before the change,
then passed with Astra. Updated bootstrap and result-ingestion fixtures exercise
the new target without rewriting frozen migration evidence. Historical
activation comparison explicitly accounts for the model difference, revision
and activation controls. Subsequent
[canary hardening](2026-09-05-canary-preflight-hardening.md) adds a separate
one-attempt Hermes bound to the same still-undeployed revision 6; the historical
activation fixture retains its previous retry setting.

`cargo test -p pip-control --tests` passed; the two explicit external/real-Hermes
tests remain ignored. Formatting and focused Clippy were also checked. This is
not an end-to-end provider test or systemd lifecycle certification.

Pirate remains on source `ab16528`, inert host policy revision 3, with its
interrupted Sol case/task preserved. This source change needs the normal
reviewed release and controlled policy/profile reconciliation. Before execution,
resolve worker-input, scratch-storage, and permanent-failure handling gaps from
the [planner investigation](2026-09-05-planner-failure-investigation.md). Model
migration does not resolve the provider's `cyber_policy` rejection or authorize
a new attempt by itself. Conversational Pip was not changed.
