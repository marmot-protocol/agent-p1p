# Repository instructions

Pip turns authorized GitHub issues into planned, built, reviewed draft PRs that
a human merges. Its job is to keep making progress safely, and to tell a human
clearly when it cannot.

## Safety properties (do not weaken)

- Humans merge. No model, worker, or configuration path may merge or enable
  autonomous merge. The MDK pilot stays in shadow merge mode until JG changes it.
- Bind CI, reviews, and final readiness to the exact PR head SHA.
- Never silently substitute models. Enforce this where it can actually be
  observed (pinned provider flags, provider model probes, profile bindings),
  not through a model's self-report.
- Workers never receive GitHub credentials, signing keys, or ledger access.
  Do not place credentials, OAuth tokens, or provider secrets in this repository.
- The Rust ledger is authoritative and append-only. Hermes Kanban is an
  execution queue and operational projection, not a second workflow database.
- Repository, issue, actor, notification, PR, and canary identities live in
  validated policy or live evidence, never compiled into the engine.

## Failure handling

Classify every failure before deciding what to do with it:

- **Transient** (provider throttling or outage, network, lease expiry, crashed
  or timed-out execution infrastructure): retry with bounded backoff. Never
  spend a work budget on it.
- **Work failure** (invalid result, failing build, reviewer findings): spend a
  per-stage budget that resets when the stage, head, or plan changes.
- **Exhausted budget, deterministic blocker, or no progress**: park the case.
  Parking posts a specific, human-readable GitHub comment (what stopped, where,
  the last error) and frees the case's capacity slot.

Every stop condition must have an ordinary online exit. A trusted human resumes,
replans, or abandons a parked case by replying `@<pip-login> resume|replan|abandon`
on the issue or PR. Recovery must not require root, stopping services, or
editing the ledger.

Do not add incident-specific recovery commands, event types, or validators. If
something got stuck, fix the general mechanism (classification, budget, park,
resume) so the whole class of problem is handled.

## Engineering practice

- The target control plane is Rust. `docs/pip-architecture-plan.md` describes
  the architecture; keep it short and update it when behavior changes.
- Use test-driven development for executable behavior. Test at the real
  boundaries: capture fixtures from actual Hermes, Cursor, and GitHub output
  rather than inventing their shapes.
- Prefer deleting a mechanism to adding a special case to it.
- Keep orchestration deterministic: the controller decides transitions from
  validated data. Models do work; they do not echo bookkeeping fields the
  controller already knows.
- Keep prompts focused on the engineering task. Controller-owned bookkeeping
  belongs in code, not in skill text.
- Commit messages explain why the change was needed, not only what changed.
- Canonical skills live under `skills/`; runtime profile directories symlink
  to them.
