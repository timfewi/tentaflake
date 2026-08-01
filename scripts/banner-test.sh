#!/usr/bin/env bash
# banner-test.sh — visual preview + self-check for the tentaflake-status banner.
#
# Replicates the statusBanner script from modules/shell.nix with a stubbed
# `systemctl` and a fake mixed-runtime fleet (active/inactive/failed), so the
# banner can be eyeballed and regression-checked on any dev machine — no
# NixOS host or agent containers needed. Keep in sync with modules/shell.nix.
#
# Usage: ./scripts/banner-test.sh [--stats | --health] [--hide]
#        (exits non-zero if a self-check fails)
set -euo pipefail

# Same flags the real banner takes: --stats is the wide `tentaflake stats`
# dashboard, --health the `tentaflake health` vitals view, --hide redacts
# everything that identifies the host.
wide=0; hide=0; health=0
for a in "$@"; do
  case "$a" in
    --stats) wide=1 ;;
    --health) health=1 ;;
    --hide|-H) hide=1 ;;
  esac
done

REPO=$(cd -- "$(dirname -- "$0")/.." && pwd)
LOGO_FILE=$REPO/public/tentaflake-shell-logo.txt
[ -f "$LOGO_FILE" ] || { echo "logo file missing: $LOGO_FILE"; exit 1; }

# ── Stubs (what the real banner gets from the host) ──
systemctl() {
  # `--failed` feeds the health view's first check; one failed unit here is what
  # drives the verdict to critical, so the escalation path is exercised.
  if [ "${1:-}" = "--failed" ]; then
    printf 'docker-hermes-flux-reporter.service loaded failed failed Hermes flux-reporter\n'
    return 0
  fi
  local unit="${*: -1}" st="inactive" since=""
  case "$unit" in
    *atlas*) st=active; since=$(LC_ALL=C date -d '-50 hours' '+%a %F %T %Z') ;;
    *data-scout*) st=active; since=$(LC_ALL=C date -d '-3 hours -12 minutes' '+%a %F %T %Z') ;;
    *log-analyst*) st=active; since=$(LC_ALL=C date -d '-45 minutes' '+%a %F %T %Z') ;;
    *flux*) st=failed ;;
  esac
  [ "${1:-}" = "is-active" ] && { printf '%s\n' "$st"; return 0; }
  printf 'ActiveState=%s\nActiveEnterTimestamp=%s\n' "$st" "$since"
}
hostname() { echo agent-hub; }
backend=docker
ts_ip=100.73.54.21
# Present so `command -v tailscale` succeeds in the health view's check.
tailscale() { printf '%s\n' "$ts_ip"; }

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

# Keep this inventory logic in sync with modules/shell.nix. Report each
# physical, non-empty disk once, preferring its root or first data-like mount.
disk_rows() {
  local lsblk_bin="${1:-lsblk}" df_bin="${2:-df}"
  local device bytes type mountpoint df_line used total pct size

  while read -r device bytes type; do
    [ "$type" = disk ] || continue
    [ "${bytes:-0}" -gt 0 ] 2>/dev/null || continue
    case "$device" in
      /dev/loop*|/dev/ram*|/dev/zram*) continue ;;
    esac

    mountpoint=$("$lsblk_bin" -nrpo MOUNTPOINT "$device" 2>/dev/null | awk '
      $0 == "/" { print; found = 1; exit }
      /^\// && $0 != "/boot" && $0 !~ /^\/nix\/store/ && candidate == "" { candidate = $0 }
      END { if (!found && candidate != "") print candidate }
    ' || true)

    if [ -n "$mountpoint" ]; then
      df_line=$("$df_bin" -Ph -- "$mountpoint" 2>/dev/null | awk '
        NR == 2 { sub(/%/, "", $5); print $3 "\t" $2 "\t" $5 }
      ' || true)
      if [ -n "$df_line" ]; then
        IFS=$'\t' read -r used total pct <<< "$df_line"
        printf '%s\t%s / %s\t%s\n' "$mountpoint" "$used" "$total" "$pct"
        continue
      fi
    fi

    size=$(numfmt --to=iec --suffix=B --format='%.1f' "$bytes" 2>/dev/null || true)
    [ -n "$size" ] || size="$bytes bytes"
    printf '%s\t%s unmounted\n' "${device##*/}" "$size"
  done < <("$lsblk_bin" -bdnrpo NAME,SIZE,TYPE 2>/dev/null || true)
}

# Render: logo left, the collected `info` column right. Every view starts with
# this, so none of them can drift on the header. The module indents the logo 2
# spaces at build time; sed replicates that.
render_header() {
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
}

# ── Fake fleet: 3 active, 2 inactive, 1 failed ──
records=$(printf 'hermes\tatlas-core\thermes-atlas-core\tdocker-hermes-atlas-core.service\t/var/lib/hermes-atlas-core\nzeroclaw\tdata-scout\tzeroclaw-data-scout\tdocker-zeroclaw-data-scout.service\t/var/lib/zeroclaw-data-scout\nhermes\tflux-reporter\thermes-flux-reporter\tdocker-hermes-flux-reporter.service\t/var/lib/hermes-flux-reporter\nhermes\tlog-analyst\thermes-log-analyst\tdocker-hermes-log-analyst.service\t/var/lib/hermes-log-analyst\nzeroclaw\tmetric-lens\tzeroclaw-metric-lens\tdocker-zeroclaw-metric-lens.service\t/var/lib/zeroclaw-metric-lens\nagent\tmain\tagent-main\tdocker-agent-main.service\t/var/lib/agent-main')

# ── Host identity, shared by every view below ──
if [ "$hide" = 1 ]; then
  host_label=redacted
  hide_note=$(printf ' %b· redacted%b' "$yellow" "$reset")
else
  host_label=$(hostname)
  hide_note=""
fi
tailnet_ip=""
if command -v tailscale >/dev/null 2>&1; then
  tailnet_ip=$(tailscale ip -4 2>/dev/null | head -n1 || true)
fi

# ── `--health`: host vitals as bars, plus the checks doctor covers ──
# Mirror of the module's render_health. `--live` is not mirrored: it is pure
# terminal behavior (alt screen + repaint) with nothing to assert on here.

# /proc/stat delta: 100% = every core busy. Costs the sample interval.
cpu_busy_pct() {
  local a b s=0.4
  a=$(awk '/^cpu /{ t = 0; for (i = 2; i <= NF; i++) t += $i; print t, $5 }' /proc/stat)
  sleep "$s"
  b=$(awk '/^cpu /{ t = 0; for (i = 2; i <= NF; i++) t += $i; print t, $5 }' /proc/stat)
  awk -v a="$a" -v b="$b" 'BEGIN {
    split(a, x); split(b, y)
    dt = y[1] - x[1]; di = y[2] - x[2]
    p = (dt > 0) ? (dt - di) * 100 / dt : 0
    printf "%d", (p < 0 ? 0 : (p > 100 ? 100 : p))
  }'
}

# One vitals row: label, bar percent, printed value, detail. Percent and
# printed value are separate arguments so °C can share the row shape.
hrow() {
  # printf pads %5s by bytes and "69°C" is multibyte — pad by characters here.
  local val=$3
  while [ "${#val}" -lt 5 ]; do val=" $val"; done
  printf '    %b%-9s%b %b%s%b %b%s%b  %b%s%b\n' \
    "$dim" "$1" "$reset" \
    "$(pct_color "$2")" "$(bar "$2" 24)" "$reset" \
    "$bold" "$val" "$reset" "$dim" "$4" "$reset"
}

render_health() {
  local cpu cores mem_t mem_a mem_u mem_p sw_t sw_f sw_u sw_p
  local temp z t pct used size verdict up
  local checks=() vitals=()
  local failed_units n_failed agents_total agents_active agents_failed unit st
  vs=0 # 0 healthy · 1 degraded · 2 critical — the worst finding wins

  # ck <level> <text> — one check line, which also raises the verdict.
  ck() {
    local sym col
    case "$1" in
      0) sym="✓"; col=$green ;;
      1) sym="▲"; col=$yellow ;;
      *) sym="✗"; col=$red ;;
    esac
    [ "$1" -gt "$vs" ] && vs=$1
    checks+=("$(printf '    %b%s%b %s' "$col" "$sym" "$reset" "$2")")
  }
  # A usage percent moves the verdict on the same thresholds pct_color paints
  # with, so a red bar and a red verdict always agree.
  rate() {
    if [ "$1" -ge 90 ]; then vs=2
    elif [ "$1" -ge 75 ] && [ "$vs" -lt 1 ]; then vs=1
    fi
  }

  cores=$(nproc 2>/dev/null || echo 1)
  cpu=$(cpu_busy_pct)
  # A busy CPU is agents doing their job, not a fault — no rate() here.
  vitals+=("$(hrow cpu "$cpu" "$cpu%" "$cores cores")")

  read -r mem_t mem_a sw_t sw_f < <(awk '
    /^MemTotal:/ { t = $2 } /^MemAvailable:/ { a = $2 }
    /^SwapTotal:/ { st = $2 } /^SwapFree:/ { sf = $2 }
    END { print int(t / 1024), int(a / 1024), int(st / 1024), int(sf / 1024) }
  ' /proc/meminfo)
  mem_u=$((mem_t - mem_a)); mem_p=0
  [ "$mem_t" -gt 0 ] && mem_p=$((mem_u * 100 / mem_t))
  rate "$mem_p"
  vitals+=("$(hrow memory "$mem_p" "$mem_p%" "$(fmt_mib "$mem_u") / $(fmt_mib "$mem_t")")")

  if [ "$sw_t" -gt 0 ]; then
    sw_u=$((sw_t - sw_f)); sw_p=$((sw_u * 100 / sw_t))
    rate "$sw_p"
    vitals+=("$(hrow swap "$sw_p" "$sw_p%" "$(fmt_mib "$sw_u") / $(fmt_mib "$sw_t")")")
  fi

  # Hottest thermal zone, when the box exposes any. Bar scale is 0-100 °C.
  temp=""
  for z in /sys/class/thermal/thermal_zone*/temp; do
    [ -r "$z" ] || continue
    t=$(cat "$z" 2>/dev/null || echo "")
    case "$t" in "" | *[!0-9]*) continue ;; esac
    t=$((t / 1000))
    if [ -z "$temp" ] || [ "$t" -gt "$temp" ]; then temp=$t; fi
  done
  if [ -n "$temp" ]; then
    vitals+=("$(hrow temp "$temp" "$temp°C" "hottest thermal zone")")
  fi

  # Real block-device filesystems only — tmpfs/overlay is noise here.
  # ponytail: first four by mount point, so a live frame keeps a fixed height;
  # widen the head if a host ever needs more.
  while read -r _ size used _ pct _mnt; do
    pct=${pct%\%}
    rate "$pct"
    vitals+=("$(hrow "$_mnt" "$pct" "$pct%" "$used / $size")")
  done < <(df -Ph 2>/dev/null | awk '$1 ~ /^\/dev\//' | sort -k6 | head -4)

  # ── Checks: the same ground `tentaflake doctor` covers, one line each ──
  failed_units=$(systemctl --failed --no-legend --plain 2>/dev/null | awk '{ print $1 }' | paste -sd' ' - || true)
  n_failed=$(printf '%s' "$failed_units" | wc -w)
  if [ "$n_failed" -gt 0 ]; then
    # Unit names carry container (so agent) names — count only when masked.
    if [ "$hide" = 1 ]; then
      ck 2 "$n_failed failed systemd unit(s)"
    else
      ck 2 "failed systemd units: $failed_units"
    fi
  else
    ck 0 "no failed systemd units"
  fi

  agents_total=0; agents_active=0; agents_failed=0
  while IFS=$'\t' read -r _ _ container unit _; do
    [ -n "$container" ] || continue
    agents_total=$((agents_total + 1))
    st=$(systemctl is-active "$unit" 2>/dev/null || true)
    case "$st" in
      active) agents_active=$((agents_active + 1)) ;;
      failed) agents_failed=$((agents_failed + 1)) ;;
    esac
  done <<< "$records"
  if [ "$agents_total" = 0 ]; then
    ck 0 "no agents defined"
  elif [ "$agents_failed" -gt 0 ]; then
    ck 2 "$agents_failed of $agents_total agents failed — tentaflake status"
  elif [ "$agents_active" -lt "$agents_total" ]; then
    ck 1 "$agents_active of $agents_total agents active"
  else
    ck 0 "all $agents_total agents active"
  fi

  if command -v tailscale >/dev/null 2>&1; then
    if [ -z "$tailnet_ip" ]; then
      ck 1 "tailscale not connected — sudo tailscale up"
    elif [ "$hide" = 1 ]; then
      ck 0 "tailscale connected"
    else
      ck 0 "tailscale connected ($tailnet_ip)"
    fi
  fi

  case "$vs" in
    0) verdict=$(printf '%b● healthy%b' "$green" "$reset") ;;
    1) verdict=$(printf '%b▲ degraded%b' "$yellow" "$reset") ;;
    *) verdict=$(printf '%b✗ critical%b' "$red" "$reset") ;;
  esac

  info=()
  info+=("$(printf '%b%btentaflake%b %b%s%b' "$bold" "$cyan" "$reset" "$bold" "$host_label" "$reset")")
  info+=("$(printf '%bhost health%b · %s%s' "$dim" "$reset" "$verdict" "$hide_note")")
  info+=("")
  kv "kernel" "$(uname -sr)"
  up=$(awk '{ print int($1) }' /proc/uptime 2>/dev/null || true)
  [ -n "$up" ] && kv "uptime" "$(fmt_dur "$up")"
  kv "load" "$(awk '{ print $1", "$2", "$3 }' /proc/loadavg 2>/dev/null || true)"
  if [ -n "$tailnet_ip" ] && [ "$hide" = 1 ]; then
    kv "tailnet" "${dim}redacted${reset}"
  elif [ -n "$tailnet_ip" ]; then
    kv "tailnet" "$tailnet_ip"
  fi

  render_header
  printf '\n  %b──────────────────────────────────────────────%b\n' "$dim" "$reset"
  printf '\n  %bVITALS%b\n' "$bold$cyan" "$reset"
  printf '%s\n' "${vitals[@]}"
  printf '\n  %bCHECKS%b\n' "$bold$cyan" "$reset"
  printf '%s\n' "${checks[@]}"
}

if [ "$health" = 1 ]; then
  out=$(render_health)
  printf '%s\n' "$out"
  # shellcheck disable=SC2001  # regex strip needs sed; ${var//…} can't do [0-9;]*
  plain=$(sed -e $'s/\x1b\[[0-9;]*m//g' <<< "$out")
  # The fixture has a failed unit and a failed agent — the worst finding must win.
  case "$plain" in
    *"host health · ✗ critical"*) ;;
    *) echo "VERDICT MISMATCH: expected critical with a failed unit + failed agent"; exit 1 ;;
  esac
  # Every vitals row carries a 24-cell bar; cpu/memory/root disk are never absent.
  for want in cpu memory /; do
    n=$(awk -v l="$want" '$1 == l && length($2) == 24 { c++ } END { print c + 0 }' <<< "$plain")
    [ "$n" -eq 1 ] || { echo "VITALS MISMATCH: no 24-cell bar row for '$want'"; exit 1; }
  done
  # --health must redact exactly as --stats does. Same child trick as below.
  if [ -z "${BANNER_TEST_CHILD:-}" ]; then
    masked=$(BANNER_TEST_CHILD=1 "$0" --health --hide 2>&1)
    leaked=""
    for s in "$(hostname)" "$ts_ip" atlas-core data-scout flux-reporter log-analyst metric-lens; do
      case "$masked" in *"$s"*) leaked="$leaked $s" ;; esac
    done
    [ -z "$leaked" ] || { echo "REDACTION LEAK (health):$leaked"; exit 1; }
  fi
  echo "health self-check OK (verdict critical, bars 24 cells, --hide clean)"
  exit 0
fi

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

while IFS=$'\t' read -r disk_name disk_usage disk_pct; do
  [ -n "$disk_name" ] || continue
  if [ -n "$disk_pct" ]; then
    kv "disk $disk_name" "$disk_usage ($(pct_color "$disk_pct")$disk_pct% used$reset)"
  else
    kv "disk $disk_name" "$disk_usage"
  fi
done < <(disk_rows)
if [ "$hide" = 1 ]; then
  kv "tailnet" "${dim}redacted${reset}"
else
  kv "tailnet" "$ts_ip"
fi

render_header
printf '\n  %b──────────────────────────────────────────────%b\n' "$dim" "$reset"

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
