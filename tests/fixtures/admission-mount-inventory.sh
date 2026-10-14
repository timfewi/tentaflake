#!/usr/bin/env bash
# Test-only context for maintenance failures. Never print raw mount sources.
set -euo pipefail
export LC_ALL=C

inventory=$(
  {
    systemctl list-units --all --type=mount --plain --no-legend --no-pager &&
      systemctl list-unit-files --type=mount --no-legend --no-pager
  } | awk '{ print $1 }' | sort -u
)
mapfile -t units <<< "$inventory"
test "${#units[@]}" -le 512
for unit in "${units[@]}"; do
  [[ ${#unit} -le 255 && $unit =~ ^[-a-zA-Z0-9_.:@\\]+\.mount$ ]]
  identity=$(systemctl show --value --property=Id -- "$unit")
  test "$identity" = "$unit"
  kind=$(systemctl show --value --property=Type -- "$unit")
  [[ ${#kind} -le 64 && $kind =~ ^[a-zA-Z0-9_.-]*$ ]]
  source=$(systemctl show --value --property=What -- "$unit")
  case "$source" in
    '') class=empty ;;
    none) class=none ;;
    /*) class=absolute ;;
    *) class=other ;;
  esac
  printf 'mount inventory: unit=%s type=%s source=%s\n' "$unit" "${kind:-empty}" "$class"
done
