# Repository instructions

- Treat `docs/pip-architecture-plan.md` as the target architecture.
- Implement the target control plane in Rust; retain Python only as a migration
  reference until parity and cutover gates pass.
- Use strict TDD for executable behavior.
- Keep orchestration deterministic and token-free.
- Keep the Rust ledger authoritative; Hermes Kanban is an execution queue and
  operational projection, not a second workflow database.
- Keep repository, issue, actor, notification, PR, and canary identities in
  validated policy or live evidence, never compiled into the generic engine.
- Never silently substitute models.
- Bind CI and reviews to exact PR head SHAs.
- Keep the MDK pilot in shadow merge mode until JG explicitly changes it.
- Do not place credentials, OAuth tokens, or provider secrets in this repository.
- Canonical skills live under `skills/`; runtime profile directories should symlink to them.
