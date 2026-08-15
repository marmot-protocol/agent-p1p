# Repository instructions

- Treat `docs/pip-v2-architecture-plan.md` as the target architecture.
- Use strict TDD for executable behavior.
- Keep orchestration deterministic and token-free.
- Never silently substitute models.
- Bind CI and reviews to exact PR head SHAs.
- Keep the MDK pilot in shadow merge mode until JG explicitly changes it.
- Do not place credentials, OAuth tokens, or provider secrets in this repository.
- Canonical skills live under `skills/`; runtime profile directories should symlink to them.
