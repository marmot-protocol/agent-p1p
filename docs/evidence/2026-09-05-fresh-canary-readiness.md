# Fresh canary preparation

Date: 2026-09-05. Preparation only; no label, case, board, runtime policy, or
execution-service change. Installed source remains `ab16528`, schema 8, inert
revision 3. Conversational Pip and webhook ingress are separate active services.

## Verified preparation

- No open MDK issue currently carries `pip-ok`; pending webhook spool is empty.
- The only board card is `t_f4473714`, the old unassigned, ready planner gate
  for abandoned `repo:1055628515#891@3`. No worker session or result exists on
  that card. It was not deleted, archived, completed, or reassigned.
- Stock Hermes's ready dispatcher skips unassigned tasks when no
  `kanban.default_assignee` is configured. The managed root has no such
  fallback. Preserve that invariant when activating; do not accidentally
  route this historical gate into a default worker.
- The live branch rule still requires `Required CI` from integration `15368`.
- The policy retains exact Sol/Grok/Kimi role bindings, the separate Opus
  shadow comparison, one-active-case limits, and shadow/human-only merge.

## Additional non-production tests

The installed implementation already ignores results whose case was revoked,
taken over, or superseded. A new test exercises all three after a planner
projection exists: no Hermes query, accepted run, ledger mutation, or new
outbox work is allowed. This is test coverage of existing behavior, not an
engine change requiring another release.

Focused results:

- result ingestion: 6 passed;
- dispatch/recovery: 9 passed, 1 explicit real-Hermes test ignored (its prior
  separate execution is recorded in the gate-free evidence);
- authorization reconciliation: 3 passed;
- ledger stale/revoked/delivered dispatch reservation regression: 1 passed;
- focused Clippy with warnings denied, formatting, and diff checks passed.

The existing mixed-review test specifically interrupts fan-out before its
acknowledgment, proves direct work is not exposed early, then recovers without
creating a duplicate Hermes reviewer or changing models. These are bounded
fixture proofs, not an actual full live reviewer run.

## Native build prerequisites

The service PATH previously lacked `just` and Clang. Installed Debian packages
`just`, `clang`, and `libclang-dev` plus their dependencies: 12 new packages,
approximately 446 MB installed; no upgrades, removals, or autoremove.

A bounded transient unit matching the direct-worker restrictions verified:

- `just 1.40.0`;
- Clang `19.1.7` compiling, linking, and executing a tiny C program;
- loading `/usr/lib/llvm-19/lib/libclang.so`;
- ledger and Hermes authentication remained inaccessible.

```text
LIBCLANG_LOAD_OK
PIP_NATIVE_TOOLS_SANDBOX_OK
```

This adds native prerequisites to the earlier Rust sandbox proof. It does not
claim full MDK CI has run on Pirate. No provider calls were made.

## Proposed issue and activation boundary

Recommend [MDK #1639](https://github.com/marmot-protocol/mdk/issues/1639),
"Conformance engine subject panics on create-group errors and loses failure artifacts".
It is unassigned and has explicit regression/report acceptance criteria. No
overlapping open PR was found. At current master
`f734b31176ad628d5e5dfcb047f80e4ec7bb826c`, the subject still invokes
`create_group_with_admins_maybe_pending` and that helper still calls
`.expect("create_group")`. This verifies the reported source path, not a new
runtime reproduction; the planner must validate scope and reproduction itself.

This is simulator error propagation, not a request to modify live cryptographic
semantics. If the planner finds sensitive or expanded work is needed, the normal
policy gates must hold it for review.

On separate approval to use that issue and start the supervised run:

1. Refresh issue/PR/label eligibility and credential/model capability evidence.
2. Keep the abandoned ledger case and unassigned gate unchanged. Recheck there
   is no default-assignee fallback; stop if the board has unexpected work.
3. Stage a new active policy revision (do not reuse the old canary's revision
   4), changing only intake pause/enable, dispatch, and revision. Keep the issue
   out of generic policy; authorization is its label. Retain one-case limits
   and shadow merge mode. Prepare an inert stop policy and evidence capture.
4. Run no-candidate/empty-queue probes and prove recurring execution timers
   before applying the single approved label.
5. Supervise the new case through planner, builder/draft PR, independent reviews,
   and final shadow recommendation. Stop on unexpected work or boundary drift.

No activation or label authorization has been inferred from preparation.
