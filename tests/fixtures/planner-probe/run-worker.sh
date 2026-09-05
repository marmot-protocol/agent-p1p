#!/bin/bash
# Run only inside a bounded transient unit with the production state hidden.
set -euo pipefail
test "$(< "$PIP_PLANNER_PROBE_ROOT/ISOLATED_PROBE")" = pip-real-planner-probe
task_id="$(< "$PIP_PLANNER_PROBE_ROOT/task-id")"
hermes kanban --board pip-isolated-planner-probe dispatch --max 1 --failure-limit 1 --json
# One dispatch only. Do not retry failed provider requests or spawn another task.
for ((attempt = 0; attempt < 180; attempt++)); do
  result="$(hermes kanban --board pip-isolated-planner-probe show "$task_id" --json)"
  status="$(printf '%s' "$result" | /usr/local/lib/hermes-agent/venv/bin/python -c 'import json,sys; print(json.load(sys.stdin)["task"]["status"])')"
  case "$status" in
    done) echo ISOLATED_PLANNER_DONE; exit 0 ;;
    blocked|cancelled|archived) echo "ISOLATED_PLANNER_STOPPED $status"; exit 1 ;;
  esac
  sleep 5
done
echo ISOLATED_PLANNER_TIMED_OUT
exit 1
