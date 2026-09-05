#!/usr/bin/env bash
# Exercise real systemd scheduling with harmless, isolated oneshot services.
set -euo pipefail
templates=${1:?timer template directory required}
scope=()
stamp_root=/var/lib/systemd/timers
if [[ ${2-} == --user ]]; then
  scope=(--user)
  stamp_root=${XDG_DATA_HOME:-$HOME/.local/share}/systemd/timers
fi
prefix=pip-timer-restart-test-$$
units=()
cleanup() {
  for unit in "${units[@]}"; do
    systemctl "${scope[@]}" stop "$unit.timer" >/dev/null 2>&1 || true
    systemctl "${scope[@]}" stop "$unit.service" >/dev/null 2>&1 || true
    if [[ -f $stamp_root/stamp-$unit.timer ]]; then
      unlink "$stamp_root/stamp-$unit.timer"
    fi
  done
}
trap cleanup EXIT

for template in "$templates"/*.timer; do
  unit=$prefix-${#units[@]}
  units+=("$unit")
  properties=()
  in_timer=false
  while IFS= read -r line; do
    case "$line" in
      \#*|\;*) continue ;;
      '[Timer]') in_timer=true; continue ;;
      '['*) in_timer=false ;;
    esac
    [[ $in_timer == true && $line == *=* ]] || continue
    case "$line" in
      Unit=*) continue ;;
      # Compress time only; preserve scheduling modes and persistence policy.
      OnBootSec=*) line=OnBootSec=1us ;;
      OnUnitInactiveSec=*) line=OnUnitInactiveSec=1s ;;
      OnUnitActiveSec=*) line=OnUnitActiveSec=1s ;;
      AccuracySec=*) line=AccuracySec=100ms ;;
      RandomizedDelaySec=*) line=RandomizedDelaySec=0 ;;
    esac
    properties+=(--timer-property="$line")
  done <"$template"
  # A short test window must not inherit the manager's one-minute coalescing.
  properties+=(--timer-property=AccuracySec=100ms)

  for round in 1 2; do
    systemd-run "${scope[@]}" --unit="$unit" --property=Type=oneshot \
      "${properties[@]}" /usr/bin/true
    first=
    complete=false
    deadline=$((SECONDS + 12))
    while (( SECONDS < deadline )); do
      current=$(systemctl "${scope[@]}" show "$unit.timer" -p LastTriggerUSecMonotonic --value)
      if [[ -n $current && $current != 0 ]]; then
        if [[ -z $first ]]; then
          first=$current
        elif [[ $current != "$first" ]]; then
          next=$(systemctl "${scope[@]}" show "$unit.timer" -p NextElapseUSecMonotonic --value)
          if [[ -n $next && $next != infinity && $next != 0 ]]; then
            test "$(systemctl "${scope[@]}" show "$unit.service" -p Result --value)" = success
            test "$(systemctl "${scope[@]}" show "$unit.service" -p ExecMainStatus --value)" = 0
            complete=true
            break
          fi
        fi
      fi
      sleep 0.1
    done
    if [[ $complete != true ]]; then
      systemctl "${scope[@]}" show "$unit.timer" \
        -p SubState -p LastTriggerUSecMonotonic -p NextElapseUSecMonotonic >&2
      echo "Timer failed recurrence: $template round=$round" >&2
      exit 1
    fi
    systemctl "${scope[@]}" stop "$unit.timer"
    # Drop transient definitions and activation timestamps, like an installer
    # reload or manager restart. Retain a legacy Persistent=true stamp.
    deadline=$((SECONDS + 5))
    until [[ $(systemctl "${scope[@]}" show "$unit.timer" -p LoadState --value) == not-found ]]; do
      (( SECONDS < deadline ))
      sleep 0.1
    done
    install -d "$stamp_root"
    touch "$stamp_root/stamp-$unit.timer"
  done
  echo "TIMER_RESTART_OK $(basename "$template")"
done
