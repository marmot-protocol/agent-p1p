# Parked cases

A case parks when Pip needs a human: it ran out of a budget, could not run for
a long time, a worker reported it was blocked, a decision is open, or nothing
happened for too long. Parked states are `ESCALATED`, `BLOCKED` and
`WAITING_HUMAN`. A parked case does no work and does not hold an issue slot.

## What Pip tells you

Pip posts a comment on the issue that says what stopped, during which stage,
on which head, and the last error it saw. The common causes:

| Cause | Meaning | Usual fix |
|---|---|---|
| `PROVIDER_UNAVAILABLE` | One stage could not run for 6 hours (throttling, quota, login, network) | Fix the provider account or host, then resume |
| `MODEL_UNAVAILABLE` | The pinned model is not available to the provider account | Fix the account, or change the policy's model, then resume |
| `PROVIDER_FAILURES` | One stage failed 3 times without a usable result | Read the error; resume with guidance, or replan |
| `WORKER_BLOCKED` | A worker stopped and gave a reason | Address the reason; resume with guidance |
| `NO_PROGRESS` | Nothing happened for the stall window; the comment names what was pending | Check the named stage (CI, a service, a GitHub permission), then resume |
| `POLICY_CHANGED` | The policy changed in a way the case did not accept | Resume to adopt the new policy, or abandon |
| Remediation rounds used up | CI or reviews kept failing after the allowed rounds | Read the findings; resume (grants 3 rounds) or replan with guidance |
| Repeated finding | A reviewer raised the same finding on 3 heads | Decide whether the finding or the fix is wrong; resume with guidance |
| Open decision | The planner or final reviewer needs a product or scope answer | Answer it in the resume comment |
| Case age | Open for 7 days | Resume to start a fresh window, or abandon |

## Commands

Reply on the issue (or the PR) as a maintainer in the policy's trusted actors.
The command goes on the first line; anything after it is guidance for the next
worker.

```text
@agent-p1p resume
The flaky integration job is fixed on master; rebase onto it.
```

- `resume` continues where the case stopped. If remediation rounds were used up
  it grants three more.
- `replan` starts again from planning.
- `abandon` stops the case. Removing and re-adding the authorization label later
  restarts it, keeping its PR.

Edited comments and comments written before the pause are ignored. Pip replies
with where it resumed, or says it is queued if the issue limit is full.

A resume waits while a builder from the case is still running. It rebinds the
case to the current policy and starts a fresh age window. Failure budgets reset
per stage; nothing about the case's history is erased.

## Budgets

| Budget | Scope | Default (MDK rev 13) |
|---|---|---|
| Work failures | Per stage (each dispatched job) | 3, with a 2 minute cooldown between attempts |
| Outage | Per stage, continuous | 6 hours |
| Remediation rounds | Per case, plus 3 per resume after exhaustion | 10 |
| Repeated finding | Distinct heads since the last replan or resume | 3 |
| Stall | Since the last event, attempt, task or evidence | Longest role runtime + 2h, at least 4h |
| Age | Since authorization or the last resume | 7 days |

Human feedback and resumes never spend a remediation round. Throttling,
outages and expired leases never spend a work-failure attempt.

## Inspecting

```sh
sudo -u pip-control /opt/pip/current/bin/pip-control status --database /var/lib/pip/ledger.db --case 'CASE_KEY'
```

The controller cycle report includes, per case, `control` (the command it
applied, ignored or is waiting on) and `control_poll` (commands found by the
fallback poll).
