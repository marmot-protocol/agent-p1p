# MDK 1639 planner failure investigation

Read-only diagnosis after the supervised canary was paused. No new provider
requests, service activation, model changes, credential changes, or Hermes
source changes were made. See [canary record](2026-09-05-canary-1639.md).

## Direct cause and retry mechanism

The preserved request diagnostic identifies `invalid_request`, code
`cyber_policy`, reason `non_retryable_client_error`, timestamp
`2026-09-05T10:30:03.755502`, model `gpt-5.6-sol`, session
`20260905_101656_cdac96`. Only error metadata and request structure were
extracted; request headers and full prompt content were not exported.

This is a provider content-policy rejection, not evidence of bad OAuth, a
missing model, or a network timeout. The same attempt had logged 35 successful
model calls. The already-started Hermes retry logged another 15 successful
calls before operator supervision stopped it. These observations do not prove
future account access or explain the classifier's exact trigger.

Official [Cybersecurity checks](https://developers.openai.com/api/docs/guides/safety-checks/cybersecurity)
document `cyber_policy`; API and Codex safeguards differ. Official
[Models and Trusted Access](https://learn.chatgpt.com/docs/cyber-safety)
acknowledge that legitimate and unrelated activity can be flagged, recommend
reviewing logs and reporting suspected false positives, and scope approved
access to the identity/service and product surface. Do not assume a different
account, alias, or product has equivalent approval. The diagnostic alone cannot
establish a false positive or that an account-wide restriction is active.

Installed stock Hermes is clean at
`29112bef099274229cadff79cdff7bf7b99c4b77`. Its conversation loop recognizes a
content-policy block, but the Kanban worker exited with code 0 without invoking
`kanban_complete` or `kanban_block`. Durable `hermes kanban runs` evidence shows
run 1 recorded as a protocol violation/crash at 10:30:11, then run 2 started.
This is more precise than interpreting the gateway's generic zombie-worker
message as a process segfault.

In stock `hermes_cli/kanban_db.py`, `detect_crashed_workers` returns such tasks
to their source phase; `_record_task_failure` applies the per-task retry limit.
Pip scheduling sets `max_retries` from `max_provider_failures` (currently 3).
Pip result ingestion treats unfinished tasks as idle until completion or the
`gave_up` circuit breaker. Thus successful controller cycles and a running card
can conceal a failed prior worker attempt. This is a failure-observability and
retry-classification gap; automatic retries are bounded, not infinite.

## Independent Pip integration gaps

1. **Planner input is incomplete.** The initial immutable bundle contains the
   authorization event and numeric identities, but no issue body, comments,
   repository locator, or controller-fetched analysis snapshot. The intake
   event payload in `crates/pip-control/src/intake.rs` and task-body construction
   in `crates/pip-controller/src/scheduling.rs` confirm this. The planner skill
   nevertheless requests live issue/comments/labels/master and related work.
   Worker credentials are intentionally absent. It attempted `gh`, failed,
   and subsequently obtained public GitHub data with its own script.
2. **Workspace instructions do not describe the deployed environment.** The
   planner skill assumes scratch or an immutable checkout, while scheduling
   passes the shared case directory. The systemd unit makes repositories and
   worktrees read-only. Fetching and writing `target/` correctly failed. The
   operator Rust runbook says to use writable scratch, but the worker has no
   explicit controller-assigned scratch/build-output contract.
3. **Scratch builds bypass managed storage.** The planner used
   `profiles/planner/cache/pip-task-t_32fd1c63/cargo-target` under the Hermes
   root. Read-only `du` measured 9.7 GiB there and 664 MiB in its Cargo home.
   These are on the root filesystem, not RAID, and outside terminal-worktree
   retirement. Root had 116 GiB available; there was no disk-full failure.
   No cache was deleted during diagnosis.
4. **Auxiliary tooling is not a coherent supported set.** `jq` is absent;
   inline script/heredoc commands encountered unattended terminal denials;
   optional Pyright installation crashed in Node/V8, and rust-analyzer was
   unavailable. The deployed `MemoryDenyWriteExecute=yes` is consistent with
   the Node permission failure, but that causal link was not separately
   reproduced. Keep optional tooling disabled or explicitly provision and test
   it; do not disable safety controls globally to suppress errors.
5. **The smoke tests did not exercise a full planner session.** They proved
   queue dispatch, result contracts and selected sandbox tools independently,
   not a planner fetching evidence, testing, producing durable artifacts and
   reporting a terminal provider failure in this exact environment. The
   request accumulated substantial source/tool output (156 input items in the
   preserved request); prompt size is an efficiency observation, not proof of
   what triggered the provider classifier.

## Recommended implementation order (not implemented)

1. Add controller-produced bounded issue/comment/source snapshots to worker
   inputs, with observation times, identities and digests. Related repository
   evidence needs a scoped read path rather than handing workers credentials.
2. Define explicit read-only source, writable per-attempt scratch, Cargo target,
   and retained result locations. Place large scratch/build output on managed
   storage with reserve checks, live-attempt exclusions and cleanup rules.
3. Align skills with those paths and supported tools. Validate the immutable
   bundle deterministically before dispatch, while retaining the worker's
   contract check. Avoid requiring ad hoc inline code for basic setup.
4. Make terminal provider rejection stop and surface immediately. Verify a
   stock-Hermes integration path using a no-provider failure fixture. A
   separate fail-fast Hermes attempt limit is a conservative option; it would
   trade automatic transient recovery for explicit controller/operator retry.
   Changing the global provider limit blindly would affect direct workers too.
   No Hermes fork is required merely to fix input/storage contracts; typed
   rejection propagation still needs an isolated adapter feasibility test.
5. Test one complete offline planner lifecycle and permanent-failure handling
   under the real service restrictions before considering another model run.
   Provider access/false-positive resolution is a separate prerequisite, not
   something these implementation changes can promise to resolve.

## Preserved state

Inert host policy revision 3 hash reverified; worker gateway MainPID 0 and
disabled, execution timers stopped. Run 2 may still display `running` in the
board because it was interrupted; its PID no longer exists. No task was reset.
The new ledger case remains PLANNING, no accepted result or PR. Old abandoned
case and card are unchanged. Checkout is clean at
`8395b97bbd75c158be8d2ae293c56c5c1211d443`. Conversational Pip and ingress remain
outside the stopped execution runtime. Do not restart the old task blindly.
