# Legacy v1 migration fixtures

This directory is a language-neutral compatibility boundary between the frozen
Python reference and the Rust implementation. JSON fixtures contain no Python
module names or executable callbacks. Rust tests should ingest the same files.

The fixture cohort begins from the Python behavior at migration baseline
`2607004`, with the baseline CI repairs made on the Rust migration branch.

Files:

- `happy-path.json` — valid planner, builder, two-reviewer, final-review, and
  exact-head join evidence.
- `invalid-contracts.json` — mutations that must fail closed.
- `transitions.json` — deterministic state/outcome oracle, including loop
  escalation boundaries.
- `test-classification.toml` — module-level initial disposition for every
  legacy test module.

Classification meanings:

- `port`: preserve the behavior directly in Rust fixtures/tests.
- `replace`: preserve the requirement but test it at the new adapter/boundary.
- `legacy-only`: documents implementation machinery that must not constrain the
  Rust architecture.
- `split`: the module contains more than one category and requires case-level
  classification before its Rust workstream begins.

These fixtures are a starting cohort, not permission to preserve known Python
bugs or compiled canary identities. Every intentional parity difference must be
documented alongside the Rust test that supersedes it.
