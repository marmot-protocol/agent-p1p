# Python-reference to Rust parity v1

This cohort makes migration differences executable rather than implicit.
`transitions.json` accounts for every frozen Python transition and every target
transition. An `equivalent` mapping must have the same source, event/outcome,
loop boundary, and target state. An `intentional-difference` entry must differ
and state the target-architecture reason. Target-only entries are explicit
controller, CI, remediation, or guarded-merge safety gates.

Rust tests also adapt every result in `legacy-v1/happy-path.json` to the target
worker contracts and apply every frozen invalid-contract mutation. A change to
either oracle that is not reflected here fails CI.

This comparison approves architecture-level semantic differences only. It is
not production activation approval and does not claim live Hermes or provider
parity.
