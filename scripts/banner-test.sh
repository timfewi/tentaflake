#!/usr/bin/env bash
# banner-test.sh — visual preview + self-check for the tentaflake-status banner.
#
# Replicates the statusBanner script from modules/shell.nix with a stubbed
# `systemctl` and a fake mixed-runtime fleet (active/inactive/failed), so the
# banner can be eyeballed and regression-checked on any dev machine — no
# NixOS host or agent containers needed. Keep in sync with modules/shell.nix.
#
# Usage: ./scripts/banner-test.sh [--stats] [--hide]
#        (exits non-zero if a self-check fails)
set -euo pipefail

# Same flags the real banner takes: --stats is the wide `tentaflake stats`
# dashboard, --hide redacts everything that identifies the host.
wide=0; hide=0
for a in "$@"; do
  case "$a" in
    --stats) wide=1 ;;
    --hide|-H) hide=1 ;;
  esac
done

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
ts_ip=100.73.54.21

# The real banner fills these from each running container's cgroup. There are
# no containers on a dev box, so pin fixtures instead — deterministic numbers
# that still exercise fmt_mib, bar, the colour thresholds and the fleet total.
declare -A mem_used=( [hermes-atlas-core]=671 [zeroclaw-data-scout]=306 [hermes-log-analyst]=1900 )
declare -A mem_max=(  [hermes-atlas-core]=1536 [zeroclaw-data-scout]=1536 [hermes-log-analyst]=2048 )
declare -A pids_of=(  [hermes-atlas-core]=31 [zeroclaw-data-scout]=29 [hermes-log-analyst]=44 )
declare -A cpu_pct=(  [hermes-atlas-core]=20.1 [zeroclaw-data-scout]=0.4 [hermes-log-analyst]=88.6 )
fleet_used=0; fleet_n=0

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

# MiB → 671M / 1.5G
fmt_mib() {
  if [ "$1" -ge 1024 ]; then awk -v m="$1" 'BEGIN { printf "%.1fG", m / 1024 }'
  else printf '%dM' "$1"; fi
}

# 0-100 percent → filled/empty block bar of the given width
bar() {
  local pct=$1 w=$2 f i out=""
  f=$((pct * w / 100)); [ "$f" -gt "$w" ] && f=$w
  for ((i = 0; i < w; i++)); do
    if [ "$i" -lt "$f" ]; then out+="█"; else out+="░"; fi
  done
  printf '%s' "$out"
}

# ── Header info (rendered right of the logo below) ──
if [ "$hide" = 1 ]; then
  host_label=redacted
  hide_note=$(printf ' %b· redacted%b' "$yellow" "$reset")
else
  host_label=$(hostname)
  hide_note=""
fi
info+=("$(printf '%b%btentaflake%b %b%s%b' "$bold" "$cyan" "$reset" "$bold" "$host_label" "$reset")")
info+=("$(printf '%bmulti-runtime agent host · %s%b%s' "$dim" "$backend" "$reset" "$hide_note")")
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
if [ "$hide" = 1 ]; then
  kv "tailnet" "${dim}redacted${reset}"
else
  kv "tailnet" "$ts_ip"
fi

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
records=$(printf 'hermes\tatlas-core\thermes-atlas-core\tdocker-hermes-atlas-core.service\t/var/lib/hermes-atlas-core\nzeroclaw\tdata-scout\tzeroclaw-data-scout\tdocker-zeroclaw-data-scout.service\t/var/lib/zeroclaw-data-scout\nhermes\tflux-reporter\thermes-flux-reporter\tdocker-hermes-flux-reporter.service\t/var/lib/hermes-flux-reporter\nhermes\tlog-analyst\thermes-log-analyst\tdocker-hermes-log-analyst.service\t/var/lib/hermes-log-analyst\nzeroclaw\tmetric-lens\tzeroclaw-metric-lens\tdocker-zeroclaw-metric-lens.service\t/var/lib/zeroclaw-metric-lens\nagent\tmain\tagent-main\tdocker-agent-main.service\t/var/lib/agent-main')

if [ -z "$records" ]; then
  printf '\n  %bAGENTS%b\n' "$bold$cyan" "$reset"
  printf '    %bnone defined — see my-agents.nix.example%b\n' "$dim" "$reset"
else
  # Mixed-runtime fleet sorted by agent name, not grouped by runtime.
  records=$(sort -t$'\t' -k2,2 <<< "$records")

  # One agent class = one color: hermes yellow, zeroclaw blue, other magenta.
  agent_rows=(); failed_names=()
  total=0; n_active=0; n_failed=0; n_inactive=0
  while IFS=$'\t' read -r runtime n container unit _; do
    [ -n "$container" ] || continue
    st=""; since=""
    while IFS='=' read -r k v; do
      case "$k" in
        ActiveState) st=$v ;;
        ActiveEnterTimestamp) since=$v ;;
      esac
    done < <(systemctl show -p ActiveState -p ActiveEnterTimestamp "$unit" 2>/dev/null || true)
    total=$((total + 1))
    # Mask the display name only — $container and $unit stay real.
    if [ "$hide" = 1 ]; then n="agent-$total"; fi
    case "$runtime" in
      hermes) rcolor=$yellow ;;
      zeroclaw) rcolor=$blue ;;
      *) rcolor=$magenta ;;
    esac

    # Resource cell, rendered last in the row because its colors make
    # the field width unprintf-able for anything following it.
    memcell=""
    u=${mem_used[$container]-}
    if [ -n "$u" ]; then
      fleet_used=$((fleet_used + u)); fleet_n=$((fleet_n + 1))
      mx=${mem_max[$container]:-0}
      if [ "$mx" -gt 0 ]; then mp=$((u * 100 / mx)); else mp=0; fi
      mc=$(pct_color "$mp")
      if [ "$wide" = 1 ]; then
        cpu=${cpu_pct[$container]:-}
        [ -n "$cpu" ] && cpu="$cpu%"
        memcell=$(printf '%b%6s%b %b%5s%b  %6s / %-5s %b%s %3d%%%b' \
          "$cyan" "$cpu" "$reset" \
          "$dim" "${pids_of[$container]:-}" "$reset" \
          "$(fmt_mib "$u")" "$(fmt_mib "$mx")" \
          "$mc" "$(bar "$mp" 8)" "$mp" "$reset")
      else
        memcell=$(printf '%b%6s%b %b%s %3d%%%b' \
          "$bold" "$(fmt_mib "$u")" "$reset" "$mc" "$(bar "$mp" 6)" "$mp" "$reset")
      fi
    fi

    case "$st" in
      active)
        n_active=$((n_active + 1))
        age=""
        if [ -n "$since" ]; then
          since_s=$(date -d "$since" +%s 2>/dev/null || true)
          [ -n "$since_s" ] && age=$(fmt_dur $(($(date +%s) - since_s)))
        fi
        agent_rows+=("$(printf '    %b●%b %-20s %b%-10s%b %b%-8s%b %b%-9s%b %s' \
          "$rcolor" "$reset" "$n" "$rcolor" "$runtime" "$reset" \
          "$rcolor" "$st" "$reset" "$dim" "$age" "$reset" "$memcell")")
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
  if [ "$wide" = 1 ]; then
    printf '      %b%-20s %-10s %-8s %-9s %6s %5s  %s%b\n' "$dim" \
      AGENT RUNTIME STATE UPTIME CPU PIDS MEMORY "$reset"
  fi
  printf '%s\n' "${agent_rows[@]}"
  if [ "$wide" = 1 ] && [ "$fleet_used" -gt 0 ]; then
    host_mib=$(awk '/^MemTotal:/ { print int($2 / 1024) }' /proc/meminfo)
    fleet_pct=$((fleet_used * 100 / host_mib))
    printf '\n    %bfleet%b %s %bacross %d agents · %b%d%%%b %bof %s host ram%b\n' \
      "$dim" "$reset" "$(fmt_mib "$fleet_used")" \
      "$dim" "$fleet_n" "$(pct_color "$fleet_pct")" "$fleet_pct" "$reset" \
      "$dim" "$(fmt_mib "$host_mib")" "$reset"
  fi
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
if ! { [ "$(fmt_mib 671)" = "671M" ] && [ "$(fmt_mib 1024)" = "1.0G" ] && [ "$(fmt_mib 1536)" = "1.5G" ]; }; then
  echo "fmt_mib MISMATCH"; exit 1
fi
# bar never overflows its width, even past 100%
if ! { [ "$(bar 50 8)" = "████░░░░" ] && [ "$(bar 0 4)" = "░░░░" ] && [ "$(bar 140 4)" = "████" ]; }; then
  echo "bar MISMATCH: $(bar 50 8) / $(bar 0 4) / $(bar 140 4)"; exit 1
fi
# only agents with a cgroup entry contribute to the fleet total
if ! { [ "$fleet_used" -eq 2877 ] && [ "$fleet_n" -eq 3 ]; }; then
  echo "FLEET MISMATCH: used=$fleet_used n=$fleet_n (want 2877 / 3)"; exit 1
fi
# logo loaded and non-trivial
if ! { [ "${#art[@]}" -ge 1 ] && [ "$w" -gt 0 ]; }; then echo "LOGO NOT LOADED"; exit 1; fi
# info column alignment: title, tagline, and kernel must start at column w+4
# shellcheck disable=SC2001  # regex strip needs sed; ${var//…} can't do [0-9;]*
plain=$(sed -e $'s/\x1b\[[0-9;]*m//g' <<< "$header")
c_title=$(awk -v h="tentaflake $host_label" 'index($0, h) {print index($0, "tentaflake"); exit}' <<< "$plain")
c_tag=$(awk '/multi-runtime/ {print index($0, "multi-runtime"); exit}' <<< "$plain")
c_kern=$(awk '/kernel/ {print index($0, "kernel"); exit}' <<< "$plain")
expected=$((w + 4))
if ! { [ "$c_title" = "$expected" ] && [ "$c_tag" = "$expected" ] && [ "$c_kern" = "$expected" ]; }; then
  echo "ALIGNMENT MISMATCH: title=$c_title tag=$c_tag kernel=$c_kern expected=$expected (w=$w)"; exit 1
fi
# --hide must leave nothing that identifies the host. Re-runs this script in
# its widest mode rather than duplicating the render; the env var stops the
# child from recursing back into this block.
if [ -z "${BANNER_TEST_CHILD:-}" ]; then
  masked=$(BANNER_TEST_CHILD=1 "$0" --stats --hide 2>&1)
  leaked=""
  for s in "$(hostname)" "$ts_ip" atlas-core data-scout flux-reporter log-analyst metric-lens; do
    case "$masked" in *"$s"*) leaked="$leaked $s" ;; esac
  done
  if [ -n "$leaked" ]; then echo "REDACTION LEAK:$leaked"; exit 1; fi
fi

echo "self-check OK (logo ${#art[@]} rows × $w cols, info column at $expected)"
