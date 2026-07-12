#!/usr/bin/env bash
# banner-test.sh — visual preview + self-check for the tentaflake-status banner.
#
# Replicates the statusBanner script from modules/shell.nix with a stubbed
# `systemctl` and a fake mixed-runtime fleet (active/inactive/failed), so the
# banner can be eyeballed and regression-checked on any dev machine — no
# NixOS host or agent containers needed. Keep in sync with modules/shell.nix.
#
# Usage: ./scripts/banner-test.sh   (exits non-zero if a self-check fails)
set -euo pipefail

REPO=$(cd -- "$(dirname -- "$0")/.." && pwd)
LOGO_FILE=$REPO/public/tentaflake-shell-logo.txt
[ -f "$LOGO_FILE" ] || { echo "logo file missing: $LOGO_FILE"; exit 1; }

# ── Stubs (what the real banner gets from the host) ──
systemctl() {
  local unit="${*: -1}" st="inactive" since=""
  case "$unit" in
    *atlas*) st=active; since=$(LC_ALL=C date -d '-50 hours' '+%a %F %T %Z') ;;
    *data-scout*) st=active; since=$(LC_ALL=C date -d '-3 hours -12 minutes' '+%a %F %T %Z') ;;
    *log-analyst*) st=active; since=$(LC_ALL=C date -d '-45 minutes' '+%a %F %T %Z') ;;
    *flux*) st=failed ;;
  esac
  printf 'ActiveState=%s\nActiveEnterTimestamp=%s\n' "$st" "$since"
}
hostname() { echo agent-hub; }
backend=docker

# ── Mirror of modules/shell.nix statusBanner from here on ──
bold=$(printf '\033[1m'); dim=$(printf '\033[2m'); reset=$(printf '\033[0m')
cyan=$(printf '\033[36m'); red=$(printf '\033[31m')
yellow=$(printf '\033[33m'); blue=$(printf '\033[34m')
green=$(printf '\033[32m'); magenta=$(printf '\033[35m')

# Info rows collected here render as a column to the right of the logo.
info=()
kv() { info+=("$(printf '%b%-10s%b %s' "$dim" "$1" "$reset" "$2")"); }

# 0-100 usage percent → green / yellow (≥75) / red (≥90)
pct_color() {
  if [ "$1" -ge 90 ]; then printf '%s' "$red"
  elif [ "$1" -ge 75 ]; then printf '%s' "$yellow"
  else printf '%s' "$green"; fi
}

# seconds → compact human duration (2d 4h / 3h 12m / 45m)
fmt_dur() {
  local s=$1 d h m
  d=$((s / 86400)); h=$((s % 86400 / 3600)); m=$((s % 3600 / 60))
  if [ "$d" -gt 0 ]; then printf '%dd %dh' "$d" "$h"
  elif [ "$h" -gt 0 ]; then printf '%dh %dm' "$h" "$m"
  else printf '%dm' "$m"; fi
}

# ── Header info (rendered right of the logo below) ──
info+=("$(printf '%b%btentaflake%b %b%s%b' "$bold" "$cyan" "$reset" "$bold" "$(hostname)" "$reset")")
info+=("$(printf '%bmulti-runtime agent host · %s%b' "$dim" "$backend" "$reset")")
info+=("")

# ── System facts ──
kv "kernel" "$(uname -sr)"
up=$(awk '{print int($1)}' /proc/uptime 2>/dev/null || true)
[ -n "$up" ] && kv "uptime" "$(fmt_dur "$up")"
kv "load"   "$(awk '{print $1", "$2", "$3}' /proc/loadavg 2>/dev/null || true)"

mem=$(free -h 2>/dev/null | awk '/^Mem:/ {print $3" / "$2}' || true)
mem_pct=$(free 2>/dev/null | awk '/^Mem:/ {printf "%d", $3*100/$2}' || true)
if [ -n "$mem" ] && [ -n "$mem_pct" ]; then
  kv "memory" "$mem ($(pct_color "$mem_pct")$mem_pct% used$reset)"
elif [ -n "$mem" ]; then
  kv "memory" "$mem"
fi

disk=$(df -Ph / 2>/dev/null | awk 'NR==2 {print $3" / "$2}' || true)
disk_pct=$(df -P / 2>/dev/null | awk 'NR==2 {sub(/%/,"",$5); print $5}' || true)
if [ -n "$disk" ] && [ -n "$disk_pct" ]; then
  kv "disk /" "$disk ($(pct_color "$disk_pct")$disk_pct% used$reset)"
elif [ -n "$disk" ]; then
  kv "disk /" "$disk"
fi
kv "tailnet" "100.73.54.21"

# ── Render: logo left, info column right ──
# The module indents the logo 2 spaces at build time; sed replicates that.
mapfile -t art < <(sed 's/^/  /' "$LOGO_FILE")
w=0
for l in "${art[@]}"; do [ "${#l}" -gt "$w" ] && w=${#l}; done
pad=2 # blank rows above the info column, for rough vertical centering
rows=${#art[@]}
[ $(( ${#info[@]} + pad )) -gt "$rows" ] && rows=$(( ${#info[@]} + pad ))
header=""
for ((i = 0; i < rows; i++)); do
  l=${art[i]-}
  j=$((i - pad))
  if [ "$j" -ge 0 ] && [ -n "${info[j]-}" ]; then
    header+=$(printf '%b%s%b%*s   %s' "$cyan" "$l" "$reset" "$((w - ${#l}))" "" "${info[j]}")$'\n'
  else
    header+=$(printf '%b%s%b' "$cyan" "$l" "$reset")$'\n'
  fi
done
printf '\n%s' "$header"

printf '\n  %b──────────────────────────────────────────────%b\n' "$dim" "$reset"

# ── Agents (fake fleet: 3 active, 2 inactive, 1 failed) ──
records=$(printf 'hermes\tatlas-core\thermes-atlas-core\tdocker-hermes-atlas-core.service\nzeroclaw\tdata-scout\tzeroclaw-data-scout\tdocker-zeroclaw-data-scout.service\nhermes\tflux-reporter\thermes-flux-reporter\tdocker-hermes-flux-reporter.service\nhermes\tlog-analyst\thermes-log-analyst\tdocker-hermes-log-analyst.service\nzeroclaw\tmetric-lens\tzeroclaw-metric-lens\tdocker-zeroclaw-metric-lens.service\nagent\tmain\tagent-main\tdocker-agent-main.service')

if [ -z "$records" ]; then
  printf '\n  %bAGENTS%b\n' "$bold$cyan" "$reset"
  printf '    %bnone defined — see my-agents.nix.example%b\n' "$dim" "$reset"
else
  # Mixed-runtime fleet sorted by agent name, not grouped by runtime.
  records=$(sort -t$'\t' -k2,2 <<< "$records")

  # One agent class = one color: hermes yellow, zeroclaw blue, other magenta.
  agent_rows=(); failed_names=()
  total=0; n_active=0; n_failed=0; n_inactive=0
  while IFS=$'\t' read -r runtime n container unit; do
    [ -n "$container" ] || continue
    st=""; since=""
    while IFS='=' read -r k v; do
      case "$k" in
        ActiveState) st=$v ;;
        ActiveEnterTimestamp) since=$v ;;
      esac
    done < <(systemctl show -p ActiveState -p ActiveEnterTimestamp "$unit" 2>/dev/null || true)
    total=$((total + 1))
    case "$runtime" in
      hermes) rcolor=$yellow ;;
      zeroclaw) rcolor=$blue ;;
      *) rcolor=$magenta ;;
    esac
    case "$st" in
      active)
        n_active=$((n_active + 1))
        age=""
        if [ -n "$since" ]; then
          since_s=$(date -d "$since" +%s 2>/dev/null || true)
          [ -n "$since_s" ] && age=$(fmt_dur $(($(date +%s) - since_s)))
        fi
        agent_rows+=("$(printf '    %b●%b %-20s %b%-10s%b %b%-8s%b %b%s%b' \
          "$rcolor" "$reset" "$n" "$rcolor" "$runtime" "$reset" \
          "$rcolor" "$st" "$reset" "$dim" "$age" "$reset")")
        ;;
      failed)
        n_failed=$((n_failed + 1)); failed_names+=("$n")
        agent_rows+=("$(printf '    %b●%b %-20s %b%-10s%b %b%s%b' \
          "$red" "$reset" "$n" "$rcolor" "$runtime" "$reset" "$red" "$st" "$reset")")
        ;;
      *)
        n_inactive=$((n_inactive + 1))
        agent_rows+=("$(printf '    %b○%b %b%-20s %-10s %s%b' \
          "$rcolor" "$reset" "$dim" "$n" "$runtime" "${st:-inactive}" "$reset")")
        ;;
    esac
  done <<< "$records"

  failed_part=""
  if [ "$n_failed" -gt 0 ]; then
    failed_part=$(printf ' · %b%d failed%b' "$red" "$n_failed" "$reset")
  fi
  printf '\n  %bAGENTS%b %b(%d · %b%d active%b%b · %d inactive%s%b)%b\n' \
    "$bold$cyan" "$reset" \
    "$dim" "$total" \
    "$green" "$n_active" "$reset" \
    "$dim" "$n_inactive" \
    "$failed_part" "$dim" "$reset"
  printf '%s\n' "${agent_rows[@]}"
  if [ "$n_failed" -gt 0 ]; then
    joined=$(printf '%s, ' "${failed_names[@]}"); joined=${joined%, }
    printf '\n    %b⚠ failed: %s — tentaflake logs %s%b\n' \
      "$red" "$joined" "${failed_names[0]}" "$reset"
  fi
fi

printf '\n  %brun %b%btentaflake%b %bto manage agents · %b%btentaflake%b %bhelp for commands%b\n\n' \
  "$dim" "$reset" "$cyan" "$reset" "$dim" "$reset" "$cyan" "$reset" "$dim" "$reset"

# ── Self-checks ──
# counters + duration formatting
if ! { [ "$total" -eq 6 ] && [ "$n_active" -eq 3 ] && [ "$n_failed" -eq 1 ] && [ "$n_inactive" -eq 2 ]; }; then
  echo "COUNTER MISMATCH: total=$total active=$n_active failed=$n_failed inactive=$n_inactive"; exit 1
fi
if ! { [ "$(fmt_dur 180000)" = "2d 2h" ] && [ "$(fmt_dur 11520)" = "3h 12m" ] && [ "$(fmt_dur 2700)" = "45m" ]; }; then
  echo "fmt_dur MISMATCH"; exit 1
fi
# logo loaded and non-trivial
if ! { [ "${#art[@]}" -ge 1 ] && [ "$w" -gt 0 ]; }; then echo "LOGO NOT LOADED"; exit 1; fi
# info column alignment: title, tagline, and kernel must start at column w+4
# shellcheck disable=SC2001  # regex strip needs sed; ${var//…} can't do [0-9;]*
plain=$(sed -e $'s/\x1b\[[0-9;]*m//g' <<< "$header")
c_title=$(awk '/tentaflake agent-hub/ {print index($0, "tentaflake"); exit}' <<< "$plain")
c_tag=$(awk '/multi-runtime/ {print index($0, "multi-runtime"); exit}' <<< "$plain")
c_kern=$(awk '/kernel/ {print index($0, "kernel"); exit}' <<< "$plain")
expected=$((w + 4))
if ! { [ "$c_title" = "$expected" ] && [ "$c_tag" = "$expected" ] && [ "$c_kern" = "$expected" ]; }; then
  echo "ALIGNMENT MISMATCH: title=$c_title tag=$c_tag kernel=$c_kern expected=$expected (w=$w)"; exit 1
fi
echo "self-check OK (logo ${#art[@]} rows × $w cols, info column at $expected)"
