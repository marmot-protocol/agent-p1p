# Rust worker result contracts

The canonical [worker-facing field guide](../skills/shared/workflow-contract/references/worker-result-contracts.md)
lives inside the shared `workflow-contract` skill so it travels with every
release and is reachable through the managed profile skill link. It describes
contract version 2 and workflow version 3, including the Hermes completion
envelope. Executable validation remains in `crates/pip-contracts/src/lib.rs`.

Workers must resolve the guide relative to the loaded skill directory, never
assume the target repository contains Pip documentation. The release also
ships a compatibility copy at `share/pip/docs/worker-result-contracts.md`.
