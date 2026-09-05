# Rust implementation status

**Snapshot date:** 2026-09-05
**Activation state:** public webhook receipt boundary enabled on Pirate;
controller credential provisioned; consumer timer, controller, workers, Hermes
gateway, and dispatch remain disabled

Release `933ec69` is installed on Pirate. Recurring timer readiness passed,
but the labeled canary stopped at a synthetic Hermes gate: stock Hermes
promoted it to `ready`, which Pip rejected. Authorization was removed, the case
was recorded as ABANDONED, and the runtime was returned to inert policy revision
3. That canary never reached a planner or full pipeline.

The gate-free refactor is now implemented in source. An isolated real-Hermes
CLI/scheduler test passed with a deterministic callback and lost-create-reply
recovery; it did not invoke a model. A subsequent isolated real Sol planner
produced a plan whose untouched durable completion was accepted exactly once
after a regression-tested Pip transport-metadata adapter fix. No Hermes fork
was needed. The production release, schema-7 ledger,
and abandoned canary evidence have not been changed. Details and remaining
proof gates:
[`evidence/2026-09-05-gate-free-dispatch-assessment.md`](evidence/2026-09-05-gate-free-dispatch-assessment.md).
The real-provider result and runtime limitations are recorded in
[`evidence/2026-09-05-real-planner-contract.md`](evidence/2026-09-05-real-planner-contract.md).
The subsequent [re-bootstrap and worker-guide fix](evidence/2026-09-05-hermes-rebootstrap.md)
passed a no-provider stock-Hermes test on Pirate. The subsequent
[service-visible Rust provision and sandbox checks](evidence/2026-09-05-rust-toolchain.md)
passed: Hermes scratch compilation and serial MDK `fs-private` tests. One
parallel MDK lock test failed; full MDK CI and native dependencies are not
certified by that bounded toolchain check.

This file is the implementation inventory. The target behavior remains defined
by [`pip-architecture-plan.md`](pip-architecture-plan.md); the migration
exit gates remain defined by [`migration-roadmap.md`](migration-roadmap.md).
An entry is `implemented` only when an executable path and its local tests
exist. Installed-host evidence is identified explicitly and is not evidence of
a completed canary.

## What exists

| Boundary | Current implementation | Evidence boundary |
|---|---|---|
| Deterministic workflow | Exhaustive Rust states, events, effects, policy-defined required reviewer-instance joins, semantic lane aggregation, exact-head binding, remediation/elapsed-time/repeated-finding/provider-failure bounds, durable escalation, and shadow-only MDK disposition | Workspace tests and frozen fixtures |
| Authoritative storage | SQLite schema v8 (production still v7), frozen dispatch batches and non-recycled create reservations, immutable webhook deliveries/events/evidence/runs/findings/detached review observations/workspace retirements, current-case projection, durable outbox, leases, immutable direct attempts, observer-preserving effect supersession, backup, migration, and crash injection | Workspace and disposable lifecycle tests |
| Intake reads | Signed configured-label webhook ingestion with delivery-ID/payload-digest replay protection and a live exact-issue re-read, plus bounded polling recovery using numeric repository/actor identity, exclusions, explicit holds, policy validation, and concurrency limits. A loopback-only `pip-ingress` service has only the webhook secret and atomically spools raw issue-event bodies; authenticated GitHub `ping` events are acknowledged without durable input. A separate bounded `pip-control` cycle revalidates and commits one pending delivery before marking it processed. Authenticated issue actions unrelated to the intake label are durably retired without a live read; configured-label events received while inactive are recorded as blocked and cannot create a case or outbox effect. | Adversarial HTTP/spool/consumer, inactive-delivery, unrelated-action, outage/replay, tamper, systemd-isolation, install/rollback, and disposable lifecycle tests; on Pirate, the isolated service, root-owned secret, Tailscale Funnel TLS path, public signed-request/replay probes, repository webhook reachability, production controller credential, empty-spool and authentic non-intake cycles, and a controlled configured-label add/live-reread/remove cycle are proven while the consumer timer remains disabled |
| Worker dispatch routing | Hermes-native roles receive ordinary parentless assigned tasks after immutable intent freeze and a one-time create reservation; exact-body/configuration reconciliation handles running/done cards and fails closed on missing/archived uncertain work; direct required roles become leased `RUN_DIRECT_WORKER` jobs; direct advisory/shadow instances become detached `RUN_DIRECT_OBSERVER` jobs that survive case advancement; both paths cross the immutable filesystem bridge to a separate worker identity | Fake-runner, reservation-race/restart/recovery, mixed-review, late-shadow, systemd-boundary, and offline integration tests; isolated stock-Hermes CLI/scheduler execution and lost-reply recovery passed with no model |
| Worker contracts | Contract v2 planner, builder, policy-defined reviewer-instance, and final-reviewer results bound to case, task, semantic role, reviewer ID, model, skills commit, plan, PR, and exact head | Contract fixtures and ingestion tests |
| Worker evidence bundles | Every projected worker receives the complete ordered ledger history at the claimed state revision, including record digests and a reproducible root digest; final review includes the atomically committed GitHub preflight | Ledger, scheduling, dispatch-command, and exact-final-preflight tests |
| CI reconciliation | Independent current and historical check/status evaluation on the ledger-bound PR head | Fixture and controller-cycle tests |
| GitHub reads/writes | Bounded REST reads plus bounded GraphQL review-thread pagination; idempotent issue comments, controller-owned draft PRs, exact-head reviews, ready-for-review mutation, and guarded merge. Reviewer Apps use RS256 JWTs to mint repository-scoped, short-lived installation tokens each active controller cycle. | Adapter, App-auth, and controller-cycle tests; Pirate has root-owned controller and reviewer credentials, verified controller identity/repository/read access, and successful installed-key token mint plus repository/issue/PR read probes for each distinct reviewer App, while MDK policy cannot enable the guarded path |
| Release/install | Signed source-bound release cohort, protected manual CI build/sign/upload workflow, exact action/image pins, artifact verification, content-addressed install, rollback, schema migration, isolated identities, hardened shadow timer, and inert active-runtime templates | Exact source `48ac1e2` passed ordinary and protected CI, independent signature and digest verification, live injected-failure rollback, protected-CI trust rotation, Pirate upgrade, idempotent reinstall, inert Hermes reconciliation, and ingress restart recovery |
| Active controller | One `controller-cycle` command that ingests Hermes and isolated direct-worker results, reconciles CI and authorization, performs polling recovery intake, enforces all operational bounds, handles takeover, publishes branches/plans/PRs/reviews/dispositions, verifies final preflight, and routes only freshly authorized effects | Local fixture and restart tests; live activation remains unauthorized |
| Final-review preflight | A durable observation effect joins the accepted plan/build and every policy-required reviewer instance, fresh issue/clarification and authorization evidence, exact numeric GitHub actor and role-stamped lane approvals, current head CI, clean draft-PR ownership/mergeability, and resolved review threads before final-review dispatch | State-machine, adapter, multi-instance fixture, drift, and restart-safe lease tests |
| Review publication | Two distinct controller-held lane credentials publish one aggregate general and one aggregate security/performance verdict on the exact head; each body names its required reviewer instances, while detached observations have no publication authority | Policy, state-machine, multi-instance aggregation, mutation, outage/retry, and exact-role fixture tests |
| Plan publication | Planner results first create a durable `PUBLISH_PLAN` effect; the controller publishes the immutable plan comment and only then applies the typed outcome that releases build, human disposition, or terminal recording | Contract, state-machine, mutation, and outage/retry tests |
| Draft PR publication | A review-ready builder result records only its clean local commit; after verified controller branch publication, the same durable effect creates or updates the stable case-owned draft PR and only then binds PR/head and releases independent CI observation | Initial/remediation identity, branch/PR outage retry, state-machine, and exact-head tests |
| Branch publication | Builder tasks receive deterministic case-owned worktree and branch assignments but no GitHub credential; the controller uses a signed askpass executable plus systemd credential-file path, pins the sole push URL, disables repository hooks/filesystem monitors/credential helpers/proxies/HTTP headers, forces TLS, validates clean branch/head state, uses exact force-with-lease, verifies the remote SHA, and only then mutates the draft PR | Credential-path isolation, scope, URL-drift, real-bare-remote, race, retry, and controller-cycle tests |
| Workspace allocation | Policy binds a canonical checkout, worktree root, artifact root, default branch, and branch prefix; dispatch fetches the policy-bound remote/default head, allocates the deterministic case worktree, and verifies exact clean branch/head state before projection | Real-Git checkout/worktree tests and production dispatch-boundary tests |
| Workspace storage lifecycle | Policy requires a dedicated mounted worktree filesystem, a 500 GiB free-space reserve, and 24-hour terminal retention. The controller retires at most one eligible terminal worktree per cycle, excludes running direct attempts, refuses dirty/colliding paths, never forces Git, preserves branches, rechecks capacity, and records retired/absent outcomes immutably in schema v7. New intake/direct work/draft publication/dispatch stop below reserve; result/finalization paths remain available. | Fake-storage lifecycle tests, real-Git dirty/idempotent retirement test, ledger eligibility/immutability tests, systemd mount contracts, and successful persistent Pirate bind-mount manual-remount and post-reboot probes |
| Hermes runtime bootstrap | `bootstrap-runtime` probes required CLI capabilities, creates/reprobes the repository board, writes only Pip-owned service-root/profile configuration, links canonical skills and shared auth, disables fallback/dangerous tools and per-profile dispatch, and verifies effective model/provider/reasoning/home values | Fake-CLI compatibility, ownership/drift, idempotency, and systemd-root tests plus a successful policy-revision-3 reconciliation on Pirate against Hermes v0.21.0 |
| Runtime isolation | Hermes workers see Hermes state plus read-only worktrees but not the ledger, direct artifacts, or provider home; direct workers use `pip-worker`, see immutable inbox/worktrees/artifacts/provider state, and cannot open the ledger, Hermes state, repository cache, or credentials | Unit-file contracts, queue convergence tests, and disposable-systemd identity/directory lifecycle |
| Guarded merge | An explicitly guarded/autonomous policy selects the merge method; the controller revalidates the complete final gate, marks the draft ready, revalidates, emits a separate merge effect, merges with expected-head protection, and verifies the recorded merge commit | Restart-convergence, shadow-disablement, state-machine, GraphQL, and mutation tests |
| Human disposition | `HOLD_FOR_HUMAN`, `ESCALATE`, and shadow-ready effects publish idempotent provenance-marked issue or draft-PR comments; local completion, block, abandonment, and takeover effects commit evidence without writing after lost authorization | Mutation fixtures and transactional effect/evidence tests |
| Provider retry control | Direct-provider failures are immutable attempts counted by Pip; Hermes tasks receive the policy retry limit and a terminal `gave_up` circuit breaker is converted to a Pip operational-bound escalation | Direct queue, Hermes projection, terminal-run, and escalation tests; live Cursor calls proved current Grok, Kimi, and Opus availability under `pip-worker`, and an isolated Kimi connection-refusal/fresh-success drill proved live direct-provider recovery without changing the production ledger |

## What is deliberately inert

- `config/target/repositories/mdk.json` has intake disabled, repository paused,
  dispatch disabled, merge mode `shadow`, and autonomous merge false. Source
  policy revision 3/workflow 3 binds the three verified numeric GitHub actors,
  the live `Required CI` ruleset context, required Sol/Kimi instances, and a
  detached Opus comparison instance without granting activation authority. It
  is installed inertly on Pirate.
- The production installer does not enable or start any reconciliation,
  gateway, controller, or direct-worker path.
- Controller and direct-worker instance templates plus the dedicated Hermes
  gateway unit are installed but inert.
- The live canary created one orphan activation-gate card before abandonment.
  It has no planner card, ledger runs, or PR. Preserve that board/ledger evidence;
  the old empty-board activation script is not a valid retry procedure.

## Remaining cutover work

The remaining gates require host-specific configuration or explicit authority;
they are not claims that local adapter tests already proved production:

The canonical checkout, service-owned Hermes auth, exact-release
`bootstrap-runtime`, dedicated Pirate workspace mount, non-dispatching GitHub
reconciliation, conversational Sol probe, direct Grok/Kimi/Opus capability
probes, isolated direct-provider outage/recovery drill, and supervised
empty-board Pip gateway probe have passed their inert real-host gates.

1. Finish the isolated gate-free dispatch/recovery proof matrix, including
   result-contract ingestion under revocation and partial reviewer fan-out.
2. The isolated real-planner/result-contract gate, stock-Hermes re-bootstrap,
   and packaged worker guide checks have passed. The service-visible Rust
   toolchain is now provisioned and sandbox-tested; retain the parallel-test
   caveat in its evidence. No Hermes fork.
3. Build and verify the candidate release and its schema-8 migration/recovery.
4. Decide explicit retirement/reconciliation and retry semantics for the
   abandoned canary and orphan gate; do not reset production history.
5. Only then authorize renewed live shadow activation.

The installed webhook boundary, controller credential, configured-label live
reread, and policy-driven reviewer release evidence are recorded in
[`evidence/2026-09-04-pirate-controller-token.md`](evidence/2026-09-04-pirate-controller-token.md)
and
[`evidence/2026-09-04-pirate-webhook-consumer.md`](evidence/2026-09-04-pirate-webhook-consumer.md),
with the prior local-bootstrap release in
[`evidence/2026-09-04-pirate-policy-driven-reviewers.md`](evidence/2026-09-04-pirate-policy-driven-reviewers.md)
and the current protected-CI release in
[`evidence/2026-09-04-protected-release-install-recovery.md`](evidence/2026-09-04-protected-release-install-recovery.md).

The installed policy revision must remain frozen while the Phase 9 case is
active. A revision mismatch fails closed. Explicit restrictive overlays and
safe nonrestrictive migration of an in-flight case are Phase 10 work and are
not claimed by the single-case canary implementation.

## Runtime topology decision

The active controller must not depend on an operator's personal
`~/.hermes`. It uses a service-owned root:

```text
pip-control system identity
  /var/lib/pip/hermes
    Kanban database and board state
    managed profiles and canonical skill links
    shared authentication links provisioned outside the release
```

Both `HERMES_HOME` and `HERMES_KANBAN_HOME` are set to that root. The compatible
Hermes gateway claims only Hermes-native tasks. Direct Cursor work is leased by
the controller, written to `/var/lib/pip/direct-queue/inbox`, executed by
the separate `pip-worker` identity, and returned through `results`; it is
never represented as a Hermes provider override. The controller alone records
the attempt and ingests the result. Real-host compatibility, process-scoped
provider recovery, and empty-board gateway supervision have passed. Both a
deterministic isolated task and a real planner/result-ingestion probe now pass;
the live full pipeline remains unproven.

The first exact-version compatibility review on 2026-09-03 targets Hermes
`v2026.8.31` at commit
`29112bef099274229cadff79cdff7bf7b99c4b77`. It corrected the adapter to use
the immutable board `slug` instead of its display name and corrected the custom
systemd gateway invocation to declare `--external-supervisor`. That pinned code
is installed root-owned on Pirate. Pip's service-owned Hermes runtime bootstrap
has now passed against that installation; the separately supervised gateway and
live capability, process-scoped outage/recovery, and empty-board supervised
gateway probes have passed. The gate-free deterministic scheduler and actual
planner/result-ingestion tests are proven separately. Post-worker bootstrap and
contract-guide packaging are now fixed; a dedicated service-visible Rust
toolchain has also passed bounded sandbox compilation checks. Full pipeline
activation remains a separate gate.

The inert Pirate installation and service-root bootstrap are recorded in
[`evidence/2026-09-03-pirate-inert-install.md`](evidence/2026-09-03-pirate-inert-install.md).

Upstream references:

- [Hermes CLI command reference](https://github.com/nousresearch/hermes-agent/blob/main/website/docs/reference/cli-commands.md)
- [Hermes Kanban guide](https://github.com/nousresearch/hermes-agent/blob/main/website/docs/user-guide/features/kanban.md)
- [Hermes profiles guide](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/profiles.md)
- [Hermes environment variables](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/reference/environment-variables.md)
- [Hermes multi-gateway notes](https://github.com/NousResearch/hermes-agent/blob/main/docs/kanban/multi-gateway.md)
- [Hermes v2026.8.31 release](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.8.31)
- [Pinned Kanban CLI source](https://github.com/NousResearch/hermes-agent/blob/29112bef099274229cadff79cdff7bf7b99c4b77/hermes_cli/kanban.py)
- [Pinned gateway parser source](https://github.com/NousResearch/hermes-agent/blob/29112bef099274229cadff79cdff7bf7b99c4b77/hermes_cli/subcommands/gateway.py)
- [GitHub GraphQL pull-request and review-thread schema](https://docs.github.com/en/graphql/reference/pulls)
- [GitHub review rules, including the self-approval prohibition](https://docs.github.com/en/pull-requests/how-tos/review-pull-requests/approving-a-pull-request-with-required-reviews)
