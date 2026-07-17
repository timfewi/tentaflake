# ────────────────────────────────────────────────────────────
# shell.nix — Interactive shell experience for SSH/console operators
#
# When you SSH into a freshly-installed Tentaflake host (over Tailscale SSH),
# this module makes the landing experience useful instead of a bare prompt:
#
#   - `tentaflake-status`  dynamic login banner (host + agent health)
#   - `tentaflake`         backend-aware CLI to drive the agent containers
#   - bash QoL             completion, history, colored prompt, aliases
#   - modern CLI tools     eza, bat, fd, ripgrep, fzf, …  (optional)
#
# Everything is generic and composable via `tentaflake.shell.*`. No domain
# config lives here — it only ever reflects whatever agents are present.
# ────────────────────────────────────────────────────────────

{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.shell;
  backend = config.tentaflake.containerBackend; # "docker" | "podman"
  hostName = config.tentaflake.hostName;
  auditdCfg = config.tentaflake.hermes-auditd;
  consoleOn = auditdCfg.enable && auditdCfg.console.enable;

  # The system flake every installed host manages itself from. Used by the
  # `rebuild` alias and the CLI's rebuild/update subcommands alike.
  flakeDir = "/etc/nixos";
  rebuildCmd = "sudo nixos-rebuild switch --flake ${flakeDir}#${hostName}";

  # Braille-art logo; single source of truth is public/tentaflake-shell-logo.txt.
  # Indented here at build time so the banner script stays a single printf.
  logo = lib.concatMapStringsSep "\n" (line: "  " + line) (
    lib.splitString "\n" (lib.removeSuffix "\n" (builtins.readFile ../public/tentaflake-shell-logo.txt))
  );

  agentContainers = config.virtualisation.oci-containers.containers;
  agentRecords = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (
      container: def:
      let
        runtime =
          if lib.hasPrefix "zeroclaw-" container then
            "zeroclaw"
          else if lib.hasPrefix "hermes-" container then
            "hermes"
          else
            "agent";
        name = lib.removePrefix "${runtime}-" container;
        # Host state dir = host side of the container's first volume mount
        # (both mkHermesAgent and mkZeroClawAgent mount stateDir first);
        # hand-rolled containers without volumes fall back to the
        # /var/lib/<container> convention (same as hermes-auditd discovery).
        stateDir =
          if def.volumes == [ ] then
            "/var/lib/${container}"
          else
            lib.head (lib.splitString ":" (lib.head def.volumes));
      in
      "${runtime}\t${name}\t${container}\t${backend}-${container}.service\t${stateDir}"
    ) agentContainers
  );

  # ── tentaflake: operator CLI for all declarative agent containers ──
  tentaflakeCli = pkgs.writeShellApplication {
    name = "tentaflake";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.diffutils
      pkgs.gawk
      pkgs.systemd
      statusBanner
    ];
    text = ''
      # Container backend, system flake dir, and console wiring are fixed at
      # build time from the tentaflake.* options.
      BACKEND="${backend}"
      FLAKE_DIR="${flakeDir}"
      AUDITD_ENABLED="${lib.optionalString auditdCfg.enable "1"}"
      CONSOLE_ENABLED="${lib.optionalString consoleOn "1"}"
      CONSOLE_ADDR="${auditdCfg.console.addr}"

      bold=$(printf '\033[1m'); dim=$(printf '\033[2m'); reset=$(printf '\033[0m')
      red=$(printf '\033[31m'); green=$(printf '\033[32m'); yellow=$(printf '\033[33m')

      agent_records() {
        printf '%s\n' ${lib.escapeShellArg agentRecords}
      }

      usage() {
        cat <<EOF
      ''${bold}Tentaflake''${reset} — manage multi-runtime agent containers (backend: ''${BACKEND})

      ''${bold}USAGE''${reset}
        tentaflake [status]            Show all agents and their state (default)
        tentaflake logs <name> [args]  Follow an agent's logs (extra journalctl args ok)
        tentaflake restart <name>      Restart an agent
        tentaflake start <name>        Start an agent
        tentaflake stop <name>         Stop an agent
        tentaflake shell <name>        Open a shell inside an agent container
        tentaflake exec <name> -- cmd  Run a command inside an agent container
        tentaflake ps                  Show all declarative agent containers
        tentaflake top                 Live filesystem-activity TUI
        tentaflake backup <name>       Snapshot an agent's state dir to a .tar.gz here
        tentaflake doctor              Host health check (exits nonzero on problems)
        tentaflake console             Agent Console URL + how to publish it on the tailnet
        tentaflake rebuild             Apply the system config (nixos-rebuild switch)
        tentaflake update              Update flake inputs, review, then rebuild
        tentaflake help                Show this help
      EOF
      }

      agent_names() {
        agent_records | awk -F '\t' 'NF == 5 { print $2 }'
      }

      record_of() {
        local query="$1"
        agent_records | awk -F '\t' -v q="$query" '$2 == q || $3 == q { print; exit }'
      }

      field_of() {
        local record
        record=$(record_of "$1")
        if [ -z "$record" ]; then
          echo "''${red}error:''${reset} unknown agent '$1'" >&2
          echo "available: $(agent_names | paste -sd' ' -)" >&2
          exit 2
        fi
        printf '%s\n' "$record" | awk -F '\t' -v n="$2" '{ print $n }'
      }

      unit_of() { field_of "$1" 4; }
      cname_of() { field_of "$1" 3; }
      statedir_of() { field_of "$1" 5; }

      require_name() {
        if [ -z "''${1:-}" ]; then
          echo "''${red}error:''${reset} missing agent name" >&2
          echo "available: $(agent_names | paste -sd' ' -)" >&2
          exit 2
        fi
      }

      cmd_status() {
        exec tentaflake-status
      }

      cmd_rebuild() {
        echo "''${bold}Applying system configuration''${reset} — sudo nixos-rebuild switch --flake $FLAKE_DIR#${hostName}"
        if ! sudo nixos-rebuild switch --flake "$FLAKE_DIR#${hostName}"; then
          echo "''${red}error:''${reset} rebuild failed — the running system is unchanged." >&2
          echo "read the error above, fix your config in $FLAKE_DIR, then re-run: tentaflake rebuild" >&2
          exit 1
        fi
      }

      cmd_update() {
        local lock="$FLAKE_DIR/flake.lock" before answer=""
        if [ ! -f "$lock" ]; then
          echo "''${red}error:''${reset} no flake.lock at $lock" >&2
          echo "this host doesn't look flake-managed from $FLAKE_DIR — nothing to update" >&2
          exit 1
        fi
        before=$(mktemp)
        cp "$lock" "$before"
        echo "''${bold}Updating flake inputs''${reset} in $FLAKE_DIR ..."
        if ! sudo nix flake update --flake "$FLAKE_DIR"; then
          rm -f "$before"
          echo "''${red}error:''${reset} 'nix flake update' failed — nothing was changed." >&2
          echo "check the message above (often a network problem), then re-run: tentaflake update" >&2
          exit 1
        fi
        if cmp -s "$before" "$lock"; then
          rm -f "$before"
          echo "Already up to date — nothing to rebuild."
          return 0
        fi
        echo
        echo "''${bold}flake.lock changes:''${reset}"
        diff -u --label before --label after "$before" "$lock" || true
        rm -f "$before"
        echo
        printf 'Rebuild now to apply the update? [y/N] '
        read -r answer || true
        case "$answer" in
          y | Y | yes | YES) cmd_rebuild ;;
          *) echo "Not rebuilding. Apply later with: tentaflake rebuild" ;;
        esac
      }

      cmd_doctor() {
        local problems=0
        ok() { printf '  %b✓%b %s\n' "$green" "$reset" "$*"; }
        bad() {
          printf '  %b✗%b %s\n' "$red" "$reset" "$*"
          problems=$((problems + 1))
        }
        note() { printf '  %b○ %s%b\n' "$dim" "$*" "$reset"; }
        fix() { printf '      %bfix:%b %s\n' "$yellow" "$reset" "$*"; }
        svc() {
          local unit="$1" st
          st=$(systemctl is-active "$unit" 2>/dev/null || true)
          if [ "$st" = "active" ]; then
            ok "$unit is active"
          else
            bad "$unit is ''${st:-unknown}"
            fix "sudo systemctl restart $unit — then inspect: journalctl -u $unit -e"
          fi
        }

        printf '%bTentaflake doctor%b — %s\n\n' "$bold" "$reset" "$(uname -n)"

        local failed_units
        failed_units=$(systemctl --failed --no-legend --plain 2>/dev/null | awk '{print $1}' | paste -sd' ' - || true)
        if [ -n "$failed_units" ]; then
          bad "failed systemd units: $failed_units"
          fix "inspect one with: journalctl -u <unit> -e"
        else
          ok "no failed systemd units"
        fi

        local disk_pct
        disk_pct=$(df -P / 2>/dev/null | awk 'NR==2 {sub(/%/,"",$5); print $5}' || true)
        if [ -n "$disk_pct" ] && [ "$disk_pct" -ge 90 ]; then
          bad "root disk is $disk_pct% full"
          fix "free space with: sudo nix-collect-garbage --delete-older-than 14d"
        else
          ok "root disk usage: ''${disk_pct:-?}%"
        fi

        if command -v tailscale >/dev/null 2>&1; then
          local ts_ip
          ts_ip=$(tailscale ip -4 2>/dev/null | head -n1 || true)
          if [ -n "$ts_ip" ]; then
            ok "tailscale connected ($ts_ip)"
          else
            bad "tailscale is not connected"
            fix "run: sudo tailscale up"
          fi
        else
          note "tailscale not installed — skipping"
        fi

        [ -n "$AUDITD_ENABLED" ] && svc hermes-auditd
        [ -n "$CONSOLE_ENABLED" ] && svc tentaflake-console

        local runtime n container unit st
        while IFS=$'\t' read -r runtime n container unit _; do
          [ -n "$container" ] || continue
          st=$(systemctl is-active "$unit" 2>/dev/null || true)
          case "$st" in
            active) ok "agent $n ($runtime) is active" ;;
            failed)
              bad "agent $n ($runtime) has failed"
              fix "tentaflake restart $n — logs: tentaflake logs $n"
              ;;
            *) note "agent $n ($runtime) is ''${st:-inactive} — start it with: tentaflake start $n" ;;
          esac
        done < <(agent_records)

        echo
        if [ "$problems" -gt 0 ]; then
          printf '%b%s problem(s) found.%b\n' "$red" "$problems" "$reset"
          exit 1
        fi
        printf '%bNo problems found.%b\n' "$green" "$reset"
      }

      cmd_console() {
        if [ -z "$CONSOLE_ENABLED" ]; then
          echo "The Agent Console is not enabled on this host."
          echo "enable it: add 'tentaflake.hermes-auditd.console.enable = true;' to your host"
          echo "config in $FLAKE_DIR, then apply it with: tentaflake rebuild"
          exit 1
        fi
        local st
        st=$(systemctl is-active tentaflake-console 2>/dev/null || true)
        echo "''${bold}Agent Console''${reset} — http://$CONSOLE_ADDR (loopback only, service: ''${st:-unknown})"
        if [ "$st" != "active" ]; then
          echo "''${yellow}warning:''${reset} tentaflake-console is ''${st:-unknown} — try: sudo systemctl restart tentaflake-console"
        fi
        echo "publish it on your tailnet:"
        echo "  tailscale serve --bg --https=9125 $CONSOLE_ADDR"
        echo "then open: https://$(uname -n).<your-tailnet>.ts.net:9125"
      }

      cmd_backup() {
        local n="$1" unit statedir parent base stamp out
        unit=$(unit_of "$n")
        statedir=$(statedir_of "$n")
        if [ ! -d "$statedir" ]; then
          echo "''${red}error:''${reset} state dir not found: $statedir" >&2
          echo "the agent may never have started — check: tentaflake status" >&2
          exit 1
        fi
        if systemctl is-active --quiet "$unit" 2>/dev/null; then
          echo "''${yellow}warning:''${reset} agent '$n' is running — files may change mid-backup." >&2
          echo "for a consistent snapshot, stop it first: tentaflake stop $n" >&2
        fi
        parent=$(dirname "$statedir")
        base=$(basename "$statedir")
        stamp=$(date -u +%Y%m%dT%H%M%SZ)
        out="./tentaflake-$n-$stamp.tar.gz"
        echo "Backing up $statedir ..."
        if ! sudo tar czf "$out" -C "$parent" "$base"; then
          echo "''${red}error:''${reset} backup failed — see the tar error above." >&2
          echo "common causes: not enough disk space here (check: df -h .) or a cancelled sudo prompt." >&2
          exit 1
        fi
        echo "''${bold}backup written:''${reset} $out ($(du -h "$out" | awk '{print $1}'))"
        echo "restore with:"
        echo "  tentaflake stop $n"
        echo "  sudo tar xzf $out -C $parent"
        echo "  tentaflake start $n"
      }

      main() {
        local sub="''${1:-status}"
        case "$sub" in
          status|ls|"") cmd_status ;;
          logs)
            require_name "''${2:-}"; local n="$2"; shift 2 || true
            # Resolve BEFORE exec — a failing $(…) inside exec's args is ignored,
            # so an unknown name would silently tail unit "".
            local u; u=$(unit_of "$n")
            exec journalctl -u "$u" -n 100 -f "$@"
            ;;
          restart|start|stop)
            require_name "''${2:-}"
            local u; u=$(unit_of "$2")
            exec sudo systemctl "$sub" "$u"
            ;;
          shell)
            require_name "''${2:-}"
            local c; c=$(cname_of "$2")
            "$BACKEND" exec -it "$c" /bin/bash 2>/dev/null \
              || exec "$BACKEND" exec -it "$c" /bin/sh
            ;;
          exec)
            require_name "''${2:-}"; local n="$2"; shift 2 || true
            [ "''${1:-}" = "--" ] && shift
            local c; c=$(cname_of "$n")
            exec "$BACKEND" exec -it "$c" "$@"
            ;;
          ps)
            # docker/podman `ps` take no positional args — select agent
            # containers via anchored name filters (multiple filters OR).
            local filters=()
            while IFS=$'\t' read -r _ _ container _; do
              [ -n "$container" ] && filters+=(--filter "name=^''${container}$")
            done < <(agent_records)
            [ "''${#filters[@]}" -gt 0 ] || exit 0
            exec "$BACKEND" ps --all "''${filters[@]}"
            ;;
          top)
            shift || true
            if ! command -v hermes-top >/dev/null 2>&1; then
              echo "''${red}error:''${reset} hermes-top not installed — enable the audit daemon:" >&2
              echo "  tentaflake.hermes-auditd.enable = true;" >&2
              exit 1
            fi
            exec hermes-top "$@"
            ;;
          backup)
            require_name "''${2:-}"
            cmd_backup "$2"
            ;;
          doctor) cmd_doctor ;;
          console) cmd_console ;;
          rebuild) cmd_rebuild ;;
          update) cmd_update ;;
          help|-h|--help) usage ;;
          *)
            echo "''${red}error:''${reset} unknown command '$sub'" >&2
            usage >&2
            exit 2
            ;;
        esac
      }

      main "$@"
    '';
  };

  hermesCompatCli = pkgs.writeShellApplication {
    name = "hermes";
    runtimeInputs = [ tentaflakeCli ];
    text = ''
      echo "note: host command 'hermes' is deprecated; use 'tentaflake'" >&2
      exec tentaflake "$@"
    '';
  };

  # ── tentaflake-status: dynamic login banner ──
  statusBanner = pkgs.writeShellApplication {
    name = "tentaflake-status";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.gawk
      pkgs.gnused
      pkgs.procps
      pkgs.systemd
    ];
    text = ''
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
      info+=("$(printf '%bmulti-runtime agent host · ${backend}%b' "$dim" "$reset")")
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

      if command -v tailscale >/dev/null 2>&1; then
        ts=$(tailscale ip -4 2>/dev/null | head -n1 || true)
        if [ -n "$ts" ]; then
          kv "tailnet" "$ts"
        else
          kv "tailnet" "''${yellow}not connected (sudo tailscale up)''${reset}"
        fi
      fi

      # ── Render: logo left, info column right ──
      # ''${#l} counts characters, not bytes (braille is multibyte) — needs the
      # UTF-8 locale NixOS sets by default.
      mapfile -t art <<< ${lib.escapeShellArg logo}
      w=0
      for l in "''${art[@]}"; do [ "''${#l}" -gt "$w" ] && w=''${#l}; done
      pad=2 # blank rows above the info column, for rough vertical centering
      rows=''${#art[@]}
      [ $((''${#info[@]} + pad)) -gt "$rows" ] && rows=$((''${#info[@]} + pad))
      printf '\n'
      for ((i = 0; i < rows; i++)); do
        l=''${art[i]-}
        j=$((i - pad))
        if [ "$j" -ge 0 ] && [ -n "''${info[j]-}" ]; then
          printf '%b%s%b%*s   %s\n' "$cyan" "$l" "$reset" "$((w - ''${#l}))" "" "''${info[j]}"
        else
          printf '%b%s%b\n' "$cyan" "$l" "$reset"
        fi
      done

      printf '\n  %b──────────────────────────────────────────────%b\n' "$dim" "$reset"

      # ── Agents ──
      records=${lib.escapeShellArg agentRecords}

      if [ -z "$records" ]; then
        printf '\n  %bAGENTS%b\n' "$bold$cyan" "$reset"
        printf '    %bnone defined — see my-agents.nix.example%b\n' "$dim" "$reset"
      else
        # Mixed-runtime fleet sorted by agent name, not grouped by runtime.
        records=$(sort -t$'\t' -k2,2 <<< "$records")

        # One agent class = one color: hermes yellow, zeroclaw blue, other magenta.
        rows=(); failed_names=()
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
              rows+=("$(printf '    %b●%b %-20s %b%-10s%b %b%-8s%b %b%s%b' \
                "$rcolor" "$reset" "$n" "$rcolor" "$runtime" "$reset" \
                "$rcolor" "$st" "$reset" "$dim" "$age" "$reset")")
              ;;
            failed)
              n_failed=$((n_failed + 1)); failed_names+=("$n")
              rows+=("$(printf '    %b●%b %-20s %b%-10s%b %b%s%b' \
                "$red" "$reset" "$n" "$rcolor" "$runtime" "$reset" "$red" "$st" "$reset")")
              ;;
            *)
              n_inactive=$((n_inactive + 1))
              rows+=("$(printf '    %b○%b %b%-20s %-10s %s%b' \
                "$rcolor" "$reset" "$dim" "$n" "$runtime" "''${st:-inactive}" "$reset")")
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
        printf '%s\n' "''${rows[@]}"
        if [ "$n_failed" -gt 0 ]; then
          joined=$(printf '%s, ' "''${failed_names[@]}"); joined=''${joined%, }
          printf '\n    %b⚠ failed: %s — tentaflake logs %s%b\n' \
            "$red" "$joined" "''${failed_names[0]}" "$reset"
        fi
      fi

      printf '\n  %brun %b%btentaflake%b %bto manage agents · %b%btentaflake%b %bhelp for commands%b\n\n' \
        "$dim" "$reset" "$cyan" "$reset" "$dim" "$reset" "$cyan" "$reset" "$dim" "$reset"
    '';
  };

  # Aliases shared across bash and zsh (interactive shells only). The ls/cat
  # set swaps to the modern tools when tentaflake.shell.tools is enabled.
  sharedAliases = {
    ".." = "cd ..";
    "..." = "cd ../..";
    grep = "grep --color=auto";
    df = "df -h";
    free = "free -h";
    cls = "clear";
    reload = "exec $SHELL"; # reload the current shell (re-reads its rc)
    rebuild = rebuildCmd; # same command `tentaflake rebuild` runs
  }
  // (
    if cfg.tools.enable then
      {
        ls = "eza --group-directories-first";
        ll = "eza -lah --group-directories-first --git";
        la = "eza -a --group-directories-first";
        tree = "eza --tree";
        cat = "bat --paging=never";
      }
    else
      {
        ls = "ls --color=auto";
        ll = "ls -alh --color=auto";
        la = "ls -A --color=auto";
      }
  )
  // lib.optionalAttrs cfg.lazygit.enable { lg = "lazygit"; };

  # Login banner — identical behavior, per-shell guard syntax. Shown once per
  # interactive SSH/console login, never on inner subshells.
  bashMotd = ''
    if [[ $- == *i* ]] && [ -z "''${TENTAFLAKE_MOTD_SHOWN:-}" ]; then
      export TENTAFLAKE_MOTD_SHOWN=1
      if [ -n "''${SSH_CONNECTION:-}" ] || { shopt -q login_shell 2>/dev/null; }; then
        tentaflake-status 2>/dev/null || true
      fi
    fi
  '';
  zshMotd = ''
    if [[ -o interactive ]] && [[ -z "''${TENTAFLAKE_MOTD_SHOWN:-}" ]]; then
      export TENTAFLAKE_MOTD_SHOWN=1
      if [[ -n "''${SSH_CONNECTION:-}" ]] || [[ -o login ]]; then
        tentaflake-status 2>/dev/null || true
      fi
    fi
  '';
in
lib.mkIf cfg.enable {
  environment.systemPackages =
    lib.optionals cfg.tentaflakeCli.enable [
      tentaflakeCli
      hermesCompatCli
    ]
    ++ lib.optional cfg.motd.enable statusBanner
    ++ lib.optional cfg.lazygit.enable pkgs.lazygit
    ++ lib.optionals cfg.tools.enable (
      with pkgs;
      [
        eza # modern ls
        bat # modern cat (syntax highlight)
        fd # modern find
        ripgrep # fast grep
        fzf # fuzzy finder
        htop # process viewer
        btop # prettier process/resource viewer
        jq # JSON
        tree # directory tree (fallback for non-eza)
        ncdu # disk usage explorer
        wget
        dnsutils # dig/host for connectivity debugging
      ]
      # tmux lives behind its own toggle (tentaflake.shell.tmux) so it can carry
      # a config and be enabled independently of the tool set.
    );

  # Aliases applied to every interactive shell (bash + zsh).
  environment.shellAliases = sharedAliases;

  # ── Login shell ──
  # Enabling zsh makes it the admin user's interactive + login shell, overriding
  # tentaflake.adminShell (which otherwise selects the shell, e.g. bash).
  users.users.${config.tentaflake.adminUser}.shell = lib.mkIf cfg.zsh.enable (lib.mkForce pkgs.zsh);

  # ── Prompt (cross-shell) ──
  # Starship gives a clean, fast prompt for both bash and zsh. When disabled we
  # install a hand-rolled colored bash prompt instead (see interactiveShellInit).
  programs.starship = lib.mkIf cfg.starship.enable {
    enable = true;
    settings = {
      add_newline = false;
      character = {
        success_symbol = "[❄](bold green)";
        error_symbol = "[❄](bold red)";
      };
    };
  };

  # ── Smart directory jumping (cross-shell) ──
  programs.zoxide = lib.mkIf cfg.zoxide.enable {
    enable = true;
  };

  # ── tmux ──

  programs.tmux = lib.mkIf cfg.tmux.enable {
    enable = true;
    clock24 = true;
    baseIndex = 1;
    escapeTime = 0;
    historyLimit = 10000;
    terminal = "tmux-256color";
    extraConfig = ''
      set -g mouse on
      set -g renumber-windows on
    '';
  };

  # ── zsh-newuser-install guard ──
  # Create a minimal ~/.zshrc for the admin user so zsh doesn't fire the
  # interactive zsh-newuser-install wizard on first login. The real config
  # lives in the NixOS-managed /etc/zshrc (programs.zsh).
  system.activationScripts.tentaflake-zshrc = lib.mkIf cfg.zsh.enable {
    text = ''
      ZSHRC="${config.users.users.${config.tentaflake.adminUser}.home}/.zshrc"
      if [ ! -f "$ZSHRC" ]; then
        touch "$ZSHRC"
        chown ${config.tentaflake.adminUser}:users "$ZSHRC"
      fi
    '';
    deps = [ "users" ];
  };

  # ── zsh (optional): Oh My Zsh + autosuggestions + syntax highlight + fzf-tab ──
  programs.zsh = lib.mkIf cfg.zsh.enable {
    enable = true;
    enableCompletion = true;
    autosuggestions.enable = true;
    syntaxHighlighting.enable = true;
    histSize = 100000;
    setOptions = [
      "HIST_IGNORE_DUPS"
      "HIST_IGNORE_SPACE"
      "SHARE_HISTORY"
      "EXTENDED_HISTORY"
    ];
    ohMyZsh = {
      enable = true;
      # No theme — Starship owns the prompt. Plugins add completions/aliases.
      plugins = [
        "git"
        "sudo"
        "systemd"
      ]
      ++ lib.optional (backend == "docker") "docker"
      ++ lib.optional (backend == "podman") "podman";
    };
    interactiveShellInit = ''
      export EDITOR="''${EDITOR:-nano}"
    ''
    + lib.optionalString cfg.tools.enable ''
      # fzf + fzf-tab (NixOS has no programs.fzf module to lean on, so source it).
      [ -f ${pkgs.fzf}/share/fzf/completion.zsh ] && source ${pkgs.fzf}/share/fzf/completion.zsh
      [ -f ${pkgs.fzf}/share/fzf/key-bindings.zsh ] && source ${pkgs.fzf}/share/fzf/key-bindings.zsh
      source ${pkgs.zsh-fzf-tab}/share/fzf-tab/fzf-tab.plugin.zsh
      zstyle ':completion:*' menu no
      zstyle ':fzf-tab:complete:cd:*' fzf-preview 'eza -1 --color=always $realpath'
    ''
    + lib.optionalString cfg.motd.enable zshMotd;
  };

  # ── Bash quality-of-life (always present — the fallback when zsh is off) ──
  programs.bash = {
    completion.enable = true;
    interactiveShellInit = ''
      # History: large, deduped, appended (not clobbered) across sessions.
      export HISTSIZE=100000
      export HISTFILESIZE=200000
      export HISTCONTROL=ignoreboth
      export HISTTIMEFORMAT='%F %T  '
      shopt -s histappend checkwinsize 2>/dev/null || true
      export EDITOR="''${EDITOR:-nano}"
    ''
    + lib.optionalString (!cfg.starship.enable) ''
      # Hand-rolled prompt: user@host:cwd (git branch) — red user when root.
      __tf_git_branch() {
        local b
        b=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || return
        printf ' (%s)' "$b"
      }
      if [ "$(id -u)" -eq 0 ]; then __tf_uc='\[\033[1;31m\]'; else __tf_uc='\[\033[1;32m\]'; fi
      PS1="''${__tf_uc}\u@\h\[\033[0m\]:\[\033[1;34m\]\w\[\033[0;33m\]\$(__tf_git_branch)\[\033[0m\]\$ "
    ''
    + lib.optionalString cfg.motd.enable bashMotd;
  };
}
