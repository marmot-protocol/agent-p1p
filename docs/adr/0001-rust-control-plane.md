# ADR 0001: Implement the target control plane in Rust

- **Status:** Accepted
- **Date:** 2026-08-20
- **Decision owner:** JG

## Context

The legacy prototype is approximately seven thousand lines of Python plus a
large safety test suite and a complex root installer. The target is no longer a
small orchestration script. It is a long-lived service that owns durable state,
Unix sockets, systemd lifecycle, subprocesses, worktrees, authenticated network
evidence, concurrency bounds, exact artifact provenance, and guarded external
transactions.

The primary maintainer strongly prefers Rust and does not want Python to become
the long-term maintenance language. Ruby was considered as a more comfortable
dynamic-language alternative.

## Decision

Implement the target runtime as a Rust workspace with a pure deterministic core
and explicit adapters for SQLite, GitHub, Hermes, provider processes, and
systemd integration.

Retain the Python implementation temporarily as a behavioral reference and
fixture source. Do not translate it mechanically or delete it before Rust
parity and cutover evidence exist.

Canonical skills remain Markdown under `skills/`. Versioned external contracts
remain language-neutral serialized data even when Rust types generate or
validate them.

## Why Rust

- Algebraic data types and exhaustive matching fit workflow states and events.
- The pure core can make illegal transitions difficult to represent.
- Ownership and explicit concurrency help with leases, process supervision,
  and bounded parallel review.
- A compiled binary simplifies deployment and removes interpreter/`PYTHONPATH`
  ambiguity.
- Cargo lockfiles and binary digests support stronger reproducibility and
  provenance than an operator-asserted source descriptor.
- Rust is suitable for long-running systemd services, SQLite, HTTP, Unix
  sockets, JSON, and subprocess adapters.
- The maintainer can review and modify the implementation confidently.

## Why not keep Python

Python remains effective for rapid orchestration prototypes, and keeping it
would avoid migration cost. It is not required by Hermes: the integration is a
CLI/JSON boundary. For this project, long-term ownership, type-safe transitions,
deployment simplicity, and system-level failure handling outweigh the rewrite
cost.

This is not a claim that Python is intrinsically unsafe. The decision is about
fitness and maintainability for this control plane.

## Why not Ruby

Ruby would improve authoring comfort for some contributors but retain a dynamic
runtime, runtime packaging, and many of the same transition/schema drift risks.
Because any move to Ruby is also a rewrite, it does not provide enough benefit
over keeping Python. Rust provides a materially different deployment and type
safety boundary.

## Consequences

Positive:

- One native release artifact and smaller installation surface.
- Compiler-enforced state/event handling.
- Clear dependency direction and testable ports/adapters.
- Better alignment with the repository owner.

Costs:

- The migration is substantial and must not be a flag-day rewrite.
- Some dynamic JSON Schema behavior will need deliberate Rust modeling.
- Compile times and cross-target release builds become operational concerns.
- The team must preserve behavior intentionally rather than assume translation
  equivalence.

## Migration constraints

- Strict TDD applies to every executable Rust behavior.
- Port safety fixtures before the behavior they constrain.
- Keep the Rust core deterministic, token-free, and free of I/O dependencies.
- Compare Rust decisions with frozen Python fixtures and recorded evidence.
- Do not dispatch from both runtimes.
- Cut over in non-dispatching shadow mode before live activation.
- Retain a recoverable Python rollback until the Rust deployment passes the
  agreed soak and lifecycle gates.
