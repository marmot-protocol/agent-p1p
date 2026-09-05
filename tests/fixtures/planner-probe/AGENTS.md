# Isolated Pip planner fixture

This repository and issue are synthetic, offline test evidence. There is no
GitHub remote. Do not fetch, publish, push, or implement the fix. Use this
checkout's HEAD as the planned base and issue.json as the complete issue record.

Read docs/worker-result-contracts.md for the exact Rust contract. Verify the
immutable evidence bundle and use the actual Hermes task ID and requested
model/skills binding. Do not fabricate a result or assume an actual model that
differs from runtime evidence. Write plan artifacts under plans/ in this
workspace, leaving tracked source and evidence unchanged.

Complete the task once using kanban_complete with the full contract object in
metadata, and then return the same JSON object. Do not merely print a plan.
