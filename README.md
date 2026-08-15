# agent-p1p

Canonical source for Pip v2: repository-scoped boards, deterministic orchestration contracts, direct Cursor worker adapters, versioned role skills, and the MDK pilot configuration.

## Status

Initial scaffold only. It does not alter the running legacy `pip` board, create the `pip-mdk` board, dispatch workers, change MDK policy, or enable autonomous merge.

## Layout

```text
config/repositories/   Repository and pilot policy
docs/                  Architecture and implementation plan
schemas/               Versioned worker/case JSON contracts
skills/                 Canonical shared and role-specific Hermes skills
src/pip_agent/          Deterministic contract helpers and future controller
tests/                  State and repository contract tests
```

## Pilot

The first target is `marmot-protocol/mdk` on board `pip-mdk`. The pilot begins with new intake disabled and merge mode set to `shadow`. Existing MDK tasks remain owned by the legacy board until explicitly migrated or completed.

## Development

```bash
uv run --dev pytest
```

See [`docs/pip-v2-architecture-plan.md`](docs/pip-v2-architecture-plan.md).
