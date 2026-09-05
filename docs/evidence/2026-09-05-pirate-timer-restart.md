# Pirate timer restart regression

## Observed activation failure

Release `ce5e5ed58f201139ff61b811550f3743ba6893f3` drained all five pending
webhooks during the 2026-09-05 06:06:59 UTC activation attempt. The superseded
MDK #891 label delivery was processed as ineligible with the label absent.
The ledger still had zero cases, runs, or outbox entries, and the board was empty.

The three execution timers started at 06:07:11–12 UTC but did not dispatch
their services during the 90-second recurrence check. Activation rolled back
to inert policy revision 3; disabled/inactive timers and gateway and the exact
inert policy hash were checked over SSH. No canary task execution is claimed.

## Reproduction and fix

A harmless, isolated user-manager timer on Pirate (systemd 257.13) reproduced
the failure with `Type=oneshot`, `OnBootSec`, `OnUnitInactiveSec`, and
`Persistent=true`. Its first activation recurred. After stopping and unloading
the transient units, creating the same timer again retained its persistence
stamp but lost the service's monotonic activation timestamps. It reported
`SubState=elapsed`, `LastTriggerUSecMonotonic=0`, and
`NextElapseUSecMonotonic=infinity`.

Pirate's three production timer stamp files were also confirmed to exist, with
modification times from 2026-09-04 16:21 UTC. They were inspected, not removed.

The [systemd v257 timer implementation](https://github.com/systemd/systemd/blob/v257/src/core/timer.c)
loads a realtime last-trigger timestamp from the persistence stamp. That can
suppress the elapsed one-time boot trigger, while no service inactivity
timestamp remains to schedule the recurring trigger.

All four monotonic timer templates now explicitly set `Persistent=false`.
Old stamp files need not be deleted: they are ignored. The direct-worker timer
also specifies `AccuracySec=1s` instead of inheriting the manager's one-minute
default. Policy, credentials, workflow transitions, and merge mode are unchanged.

## Regression coverage

`tests/lifecycle/timer-restart.sh` exercises all packaged timer definitions with
isolated `/usr/bin/true` oneshot services. Only durations are compressed. It
requires two firings and a finite next run both before and after unloading and
recreating each timer, retaining a legacy persistence stamp for the second run.
It cleans up its own units and stamp files and never activates Pip services.
The test runs in the disposable systemd lifecycle job, including the protected
deployment workflow. It can also run against an available user manager:

```bash
bash tests/lifecycle/timer-restart.sh packaging/systemd --user
```

Before the fix, the runtime regression failed on the second controller-timer
activation with the exact stuck state above. Rust contract tests also failed
before the persistence and direct-worker accuracy changes. After the fix, all
four templates passed both rounds against Pirate's unprivileged user manager
and the disposable system manager; the seven Rust systemd contract tests,
full locked Rust workspace tests, Clippy with warnings denied, and formatting
checks passed. Live production
activation remains a separate gate after deployment of the corrected release.
