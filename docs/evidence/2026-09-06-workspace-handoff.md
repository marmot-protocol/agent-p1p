# Controller-to-worker workspace handoff repair

## Failure and live boundary

MDK #1228 reached `READY_TO_BUILD` after its accepted planner result. Its three
direct attempts failed before provider inference: the `pip-worker` process
could not enter the controller-owned `0700` case directory. The linked
worktree's Git metadata also lived under the canonical repository, which the
worker unit intentionally hides. Changing that directory's mode alone would
not repair the Git boundary.

This repair did not activate the live runtime, reset attempts, replace models,
rewrite the accepted plan, or modify the existing case workspace. The installed
runtime remains paused on source `21dc6da1f7643c27f80d5615bebf5ed096512232` and
inert policy revision 6. The conversational gateway and ingress are unaffected.

## Implemented boundary

- New case repositories have local Git metadata and independent objects. The
  controller's private canonical cache stays inaccessible to the worker.
- Initial source/Git permissions and the direct worker's umask permit the
  two service accounts to hand back edits and commits without world access.
  Worker artifacts are controller-group readable; provider-home stays private.
- Git trust is scoped to the exact case path, not a wildcard/global exception.
- Before reuse, retirement and authenticated publication, credential-free
  validation rejects linked/alternate metadata and unapproved local Git
  configuration, including filters, includes, rewrites and upload-pack hooks.
  Controller Git commands disable filesystem monitors as well as hooks.
- Terminal retirement retains the exact local branch in the canonical cache
  without force before removing the clean case tree. Conflicting branches,
  dirty workspaces and invalid metadata remain untouched.
- Legacy workspaces are rejected for execution, not silently converted. Their
  original linked-worktree retirement path remains available.

## Verification

Strict regression-first development reproduced the missing workspace API,
missing cross-UID Git environment, inaccessible artifact modes, lost retained
branch, executable filesystem-monitor setting, unsafe configuration acceptance,
and authenticated-publisher admission of unsafe metadata before repairing them.

Local Rust gates: **314 passed, zero failures, six explicitly external tests
ignored**; formatting and all-target/all-feature Clippy with warnings denied
passed. Supply-chain pins, Node 24 action pins and release-workflow contracts
passed. The new ignored identity test runs explicitly in the systemd lifecycle
gate; it is not claimed as executed by the ordinary Rust test command.

The legacy Python suite was also attempted on macOS. A shorter temporary path
removed its Unix-socket path-length failures, leaving six Linux-only peer
credential tests failing because macOS has no `socket.SO_PEERCRED`. No Python
code was changed or those tests waived; the Linux CI job remains the relevant
gate for that migration-reference suite.

The disposable Linux/systemd lifecycle test used distinct `pip-control` and
`pip-worker` UIDs and the packaged worker unit's sandbox restrictions. It:

1. Prepared a fixture as the controller under private umask settings.
2. Reproduced the original worker `200/CHDIR` failure with a `0700` case root.
3. Ran the worker successfully with repaired case permissions and umask.
4. Denied access to the canonical repository, ledger and Hermes state, with
   network access disabled for the fixture.
5. Edited existing source, created new directories/files, committed, ran Git
   integrity/status checks, and handed the result back to the controller.
6. Verified distinct UIDs, controller access to worker-created files, unchanged
   canonical base, and clean retirement with the branch retained.

It returned `WORKSPACE_TWO_UID_SANDBOX_HANDOFF_OK`. The surrounding clean
install, reinstall, upgrade, injected rollback and restart-recovery checks also
passed with runtime timers disabled. This is offline boundary evidence, not a
successful real-provider builder or end-to-end canary.

## Remaining live step

Build/sign the exact committed source through the automatic release workflow,
then verify and deploy its cohort while paused. The old #1228 workspace and
three exhausted startup attempts need an explicit history-preserving recovery;
installing the release is not permission to reset or replay them. Do not reuse
the old activation script unchanged or widen the worker's private-cache access.
