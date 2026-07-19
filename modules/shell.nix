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
  auditdCfg = config.tentaflake.auditd;
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
        # /var/lib/<container> convention (same as tentaflake-auditd discovery).
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
      pkgs.gnugrep # env-file key matching in env_value + selftest assertions
      pkgs.jq
      pkgs.systemd
      pkgs.util-linux # lsblk/mount/umount/findmnt for USB key import
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

      # Agent config (non-secret) lives in a git-tracked JSON; API keys live ONLY
      # in root-owned 0600 files under SECRET_BASE — never in this JSON or git.
      AGENTS_JSON="$FLAKE_DIR/agents.json"
      SECRET_BASE="/var/lib/tentaflake/secrets"

      bold=$(printf '\033[1m'); dim=$(printf '\033[2m'); reset=$(printf '\033[0m')
      red=$(printf '\033[31m'); green=$(printf '\033[32m'); yellow=$(printf '\033[33m')
      cyan=$(printf '\033[36m')
      # Gate colour once, globally: NO_COLOR, a non-tty stderr, or a terminal
      # that cannot render. Blanking the vars degrades every later printf with
      # no per-callsite `if`.
      if [ -n "''${NO_COLOR:-}" ] || [ ! -t 2 ] || [ "''${TERM:-dumb}" = dumb ]; then
        bold=""; dim=""; reset=""; red=""; green=""; yellow=""; cyan=""
      fi
      # fd 3 is the pinned header's write channel. Default /dev/null so hdr_emit
      # is always safe; hdr_capable reopens it on /dev/tty when pinning is on.
      # NOT stdout: ask()/pick() answers are captured with $(...) and one stray
      # escape corrupts them. NOT stderr: it may be redirected to a log.
      exec 3>/dev/null

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
        tentaflake agent list          List configured agents (from agents.json)
        tentaflake agent add           Add + configure an agent — interactive wizard, no Nix
        tentaflake agent set-model <n> Change an agent's model (interactive)
        tentaflake agent remove <n>    Remove an agent from agents.json
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

        [ -n "$AUDITD_ENABLED" ] && svc tentaflake-auditd
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
          echo "enable it: add to your host config in $FLAKE_DIR:"
          # console lives inside the auditd module — without auditd the console
          # option alone is a no-op, so print both when auditd is off too.
          [ -n "$AUDITD_ENABLED" ] || echo "  tentaflake.auditd.enable = true;"
          echo "  tentaflake.auditd.console.enable = true;"
          echo "then apply it with: tentaflake rebuild"
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
        # root's default umask leaves the tarball world-readable — it holds
        # agent secrets (auth.json, .env, keys). Own it to the caller, 0600.
        sudo chown "$(id -u):$(id -g)" "$out"
        chmod 600 "$out"
        echo "''${bold}backup written:''${reset} $out ($(du -h "$out" | awk '{print $1}'))"
        echo "restore with:"
        echo "  tentaflake stop $n"
        echo "  sudo tar xzf '$out' -C '$parent'"
        echo "  tentaflake start $n"
      }

      # ── agent config wizard (edits agents.json; keys go to root 0600 files) ──
      #
      # agents.json holds ONLY non-secret config (names/models/providers/ports/
      # envFile paths). The API key never touches this JSON, argv, or history —
      # it is read with `read -rs` (or lifted verbatim out of a file on a USB
      # stick, never sourced) and written by root to a 0600 file.
      #
      # The jq mutation primitives (aj_*) take the JSON path as $1 so they can be
      # smoke-tested against a scratch file. Manual check:
      #   tentaflake agent __selftest   # round-trips a temp agents.json, asserts

      agents_json_ensure() {
        [ -f "$AGENTS_JSON" ] || printf '%s\n' '{"hermes":[],"zeroclaw":[]}' > "$AGENTS_JSON"
      }

      # env var name a provider expects its key under (hermes/openai-style).
      provider_key_var() {
        case "$1" in
          openrouter) printf 'OPENROUTER_API_KEY' ;;
          anthropic)  printf 'ANTHROPIC_API_KEY' ;;
          openai)     printf 'OPENAI_API_KEY' ;;
          *)          : ;; # custom → caller asks for the var name (empty)
        esac
      }

      aj_has() { jq -e --arg n "$2" 'any(.hermes[],.zeroclaw[]; .name==$n)' "$1" >/dev/null 2>&1; }
      aj_append() {
        local f="$1" rt="$2" entry="$3" tmp; tmp=$(mktemp)
        jq --arg rt "$rt" --argjson e "$entry" '.[$rt] += [$e]' "$f" > "$tmp" && mv "$tmp" "$f"
      }
      aj_set_model() {
        local f="$1" n="$2" m="$3" tmp; tmp=$(mktemp)
        jq --arg n "$n" --arg m "$m" '(.hermes,.zeroclaw) |= map(if .name==$n then .model=$m else . end)' \
          "$f" > "$tmp" && mv "$tmp" "$f"
      }
      aj_remove() {
        local f="$1" n="$2" tmp; tmp=$(mktemp)
        jq --arg n "$n" '(.hermes,.zeroclaw) |= map(select(.name!=$n))' "$f" > "$tmp" && mv "$tmp" "$f"
      }
      aj_get() { jq -r --arg n "$2" 'first((.hermes[],.zeroclaw[]) | select(.name==$n) | .'"$3"')' "$1"; }
      aj_runtime() { jq -r --arg n "$2" 'if any(.hermes[];.name==$n) then "hermes" else "zeroclaw" end' "$1"; }

      # prompt on stderr, answer on stdout (usable in $(...)). Optional default.
      ask() {
        local prompt="$1" def="''${2:-}" ans
        if [ -n "$def" ]; then printf '%s [%s]: ' "$prompt" "$def" >&2
        else printf '%s: ' "$prompt" >&2; fi
        read -r ans || { echo "''${red}error:''${reset} unexpected end of input" >&2; return 1; }
        [ -n "$ans" ] || ans="$def"
        printf '%s' "$ans"
      }

      # numbered menu; chosen option on stdout. Loops until a valid pick or EOF.
      # On EOF it returns 1, which under errexit aborts the `x=$(pick …)` caller
      # — a piped run must not spin forever on an exhausted stdin.
      pick() {
        local prompt="$1"; shift
        local opts=("$@")
        local i choice
        for i in "''${!opts[@]}"; do printf '  %d) %s\n' "$((i + 1))" "''${opts[i]}" >&2; done
        while :; do
          printf '%s [1-%d]: ' "$prompt" "''${#opts[@]}" >&2
          read -r choice || { echo "''${red}error:''${reset} unexpected end of input" >&2; return 1; }
          if [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "''${#opts[@]}" ]; then
            printf '%s' "''${opts[$((choice - 1))]}"; return
          fi
          echo "''${red}invalid choice''${reset}" >&2
        done
      }

      # ── pinned header: the terminal's own scroll region (DECSTBM) ──────────
      # Paint the logo into the top N rows once, then CSI N+1;LINES r confines
      # all scrolling below it. No curses, no tput: the installed console is
      # kmscon, TERM=kmscon has no terminfo entry and `tput lines` exits 3
      # there. Raw escapes + `stty size` (an ioctl) work on kmscon, the legacy
      # VT and over SSH alike.
      #
      # ponytail: painted once, no SIGWINCH handling. A WINCH trap would EINTR
      # the `read` in ask()/pick(), which returns the default and silently eats
      # the user's answer — worse than a stale header. Upgrade path: repaint at
      # the top of each step if resizing mid-wizard ever matters.
      HDR_ROWS=0
      HDR_LINES=0
      TF_TMP=""
      TF_MOUNTS=()
      declare -A TF_VOLNAME=()
      TF_CAND=()
      TF_ROOTS=()
      TF_CLEANED=""

      # shellcheck disable=SC2059  # $1 is our own literal format, never user input
      hdr_emit() { local f="$1"; shift; printf "$f" "$@" >&3 2>/dev/null || true; }

      # Split out so __selftest can assert the degradation rules without a pty.
      hdr_capable() {
        [ -z "''${NO_COLOR:-}" ] || return 1
        case "''${TERM:-dumb}" in dumb | "") return 1 ;; esac # must NOT reject kmscon
        [ -t 0 ] && [ -t 2 ] || return 1
        [ -r /dev/tty ] && [ -w /dev/tty ] || return 1
        exec 3>/dev/tty
      }

      # Header height for a given LINES/COLUMNS. Pure — the testable bit.
      # 15 = 14 braille logo rows + rule, and needs 24 lines to leave 9 rows to
      # work in. ponytail: two sizes only; a cropped mid-size logo is the
      # upgrade path if 12..23-row terminals turn out to be common.
      hdr_plan() {
        local big=15
        # The legacy VT caps at 512 glyphs and has no braille block (see
        # modules/locale.nix) — and the live ISO deliberately runs without
        # kmscon, so the full logo would be 14 rows of boxes pinned in place.
        case "''${TERM:-}" in linux | vt* | ansi) big=2 ;; esac
        if [ "$1" -ge 24 ] && [ "$2" -ge 60 ]; then printf '%s' "$big"
        elif [ "$1" -ge 12 ] && [ "$2" -ge 40 ]; then printf 2
        else printf 0; fi
      }

      # Rows a discovery menu may use: total − header − banner − "type it
      # myself" − prompt. A DECSTBM region has NO scrollback, so anything that
      # scrolls past the top of it is gone for good.
      # ponytail: floor of 3 so a headerless/tiny run still shows something.
      cand_room() { local r=$(( $1 - $2 - 4 )); [ "$r" -ge 3 ] && printf '%s' "$r" || printf 3; }

      # tr is byte-oriented and would mangle the multibyte rule char.
      hdr_rule() { local s; s=$(printf '%*s' "$1" ""); printf '%s' "''${s// /─}"; }

      hdr_init() {
        local s l c rows i art=()
        hdr_capable || return 0
        s=$(stty size < /dev/tty 2>/dev/null) || return 0
        [[ "$s" =~ ^[0-9]+\ [0-9]+$ ]] || return 0
        l=''${s% *}; c=''${s#* }
        HDR_LINES=$l
        rows=$(hdr_plan "$l" "$c")
        [ "$rows" -gt 0 ] || return 0
        HDR_ROWS=$rows
        hdr_emit '\033[?25l\033[2J\033[H'
        if [ "$rows" -ge 15 ]; then
          mapfile -t art <<< ${lib.escapeShellArg logo}
          for i in "''${!art[@]}"; do hdr_emit '%s%s%s\n' "$cyan" "''${art[i]}" "$reset"; done
        else
          hdr_emit '  %stentaflake%s %sagent setup%s\n' "$bold" "$reset" "$dim" "$reset"
        fi
        hdr_emit '%s%s%s\n' "$dim" "$(hdr_rule "$c")" "$reset"
        # ?25h: the hide above only suppresses caret flicker while the header is
        # painted. Every prompt after this is typed by a human who needs to see
        # a cursor; tf_cleanup's ?25h stays as the Ctrl-C/error restore.
        hdr_emit '\033[%d;%dr\033[%d;1H\033[J\033[?25h' "$((rows + 1))" "$l" "$((rows + 1))"
      }

      tf_umount_all() {
        local m
        for m in "''${TF_MOUNTS[@]-}"; do
          [ -n "$m" ] || continue
          sudo umount "$m" 2>/dev/null || sudo umount -l "$m" 2>/dev/null || true
          rmdir "$m" 2>/dev/null || true
        done
        TF_MOUNTS=()
      }

      # The ONLY thing that restores the terminal and releases the stick across
      # cmd_agent_add's exit paths, a failed sudo, and Ctrl-C. Idempotent. Must
      # end with a success status: under errexit a trap whose last command fails
      # would change the script's exit status.
      # shellcheck disable=SC2329  # invoked from the traps below
      tf_cleanup() {
        [ -n "$TF_CLEANED" ] && return 0
        TF_CLEANED=1
        # The staging temp holds the plaintext key. It must not outlive an
        # abort, an errexit, or a Ctrl-C at the sudo password prompt.
        [ -n "$TF_TMP" ] && { rm -f "$TF_TMP"; TF_TMP=""; }
        tf_umount_all
        if [ "$HDR_ROWS" -gt 0 ]; then
          # 999 clamps to the real bottom row, so teardown needs no stored
          # geometry and survives a resize.
          hdr_emit '\033[r\033[999;1H\033[?25h\n'
          HDR_ROWS=0
        fi
        [ -t 0 ] && { stty echo 2>/dev/null || true; } # `read -rs` may have died mid-read
        return 0
      }

      trap 'tf_cleanup' EXIT
      trap 'tf_cleanup; exit 130' INT
      trap 'tf_cleanup; exit 143' TERM
      trap 'tf_cleanup; exit 129' HUP

      # ── key import from whatever stick is plugged in ───────────────────────
      # No filesystem label and no filename convention required: the wizard
      # searches, it does not demand an `mkfs -L`. The TENTAFLAKE_ENV label is
      # NOT deprecated — it stays the deterministic marker for the live ISO's
      # non-interactive boot (installer/firstboot.nix), which cannot prompt
      # anybody. Here it only ranks a volume first.
      #
      # NOTHING found on removable media is ever sourced, eval'd or executed.
      # Values are matched with a regex and taken as literal text.

      # Strict, and deliberately so: this gates UNATTENDED discovery on removable
      # media, where nothing is there to confirm a guess. The typed path treats
      # it as a heuristic and lets a human override it — a self-hosted gateway
      # token is allowed to look like anything.
      plausible_key() {
        local v="$1"
        [ "''${#v}" -ge 16 ] && [ "''${#v}" -le 512 ] || return 1
        [[ "$v" =~ ^[A-Za-z0-9_./:+=~-]+$ ]]
      }

      # The one non-overridable reject: control characters and newlines. Paste
      # can carry them and they are what the security boundary actually cares
      # about.
      sane_key() {
        [ -n "$1" ] && [ "''${#1}" -le 512 ] || return 1
        [[ "$1" =~ ^[[:graph:]]+$ ]]
      }

      # Enough to recognise the key, never enough to leak it. $1 is a bash
      # function argument, not an execve argv — it never reaches /proc.
      mask_key() {
        local k="$1" n=''${#1}
        if [ "$n" -le 12 ]; then printf '%s (%d chars)' "''${k//?/*}" "$n"
        else printf '%s…%s (%d chars)' "''${k:0:6}" "''${k: -4}" "$n"; fi
      }

      key_hint() {
        case "$1" in
          sk-or-*)  printf ' · looks like an OpenRouter key' ;;
          sk-ant-*) printf ' · looks like an Anthropic key' ;;
          sk-*)     printf ' · looks like an OpenAI key' ;;
        esac
      }

      # $1=file $2=varname. docker --env-file syntax, the same format the
      # live-ISO sticks already use. `export` and quotes tolerated, CRLF too.
      env_value() {
        local v
        v=$(LC_ALL=C grep -a -m1 -E "^[[:space:]]*(export[[:space:]]+)?$2=" "$1" 2>/dev/null) || return 1
        v=''${v#*=}; v=''${v%$'\r'}
        v=''${v#[\"\']}; v=''${v%[\"\']}
        plausible_key "$v" || return 1
        printf '%s' "$v"
      }

      # A .txt/.key someone saved straight out of the provider's website.
      bare_value() {
        local v
        [ "$(LC_ALL=C wc -l < "$1")" -le 1 ] || return 1
        v=$(LC_ALL=C tr -d '\r\n' < "$1")
        # `=` is in the plausible-key charset, so a malformed `KEY=` line would
        # otherwise pass as a bare token. Keep this reject.
        [[ "$v" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]] && return 1
        plausible_key "$v" || return 1
        printf '%s' "$v"
      }

      # Cheap places first: on a desktop the stick is already auto-mounted and
      # NO privileged mount happens at all. Labelled volumes sort first.
      usb_scan_roots() {
        local d lbl
        for d in /run/media/*/* /media/*/* /media/* /mnt/*; do
          [ -d "$d" ] && [ ! -L "$d" ] || continue
          # /media/* also matches the per-user dir of the /media/$USER/$LABEL
          # layout (Ubuntu/Mint udisks2), which /media/*/* already covers — that
          # double hit turned one stick into two identical candidates and killed
          # the friendly single-candidate [Y/n] path. Only a real mountpoint
          # qualifies as a bare /media/* root. Note `case` lets * match `/`, so
          # the deeper pattern must be listed first.
          case "$d" in
            /media/*/*) ;;
            /media/*) [ "$(findmnt -no TARGET --target "$d" 2>/dev/null)" = "$d" ] || continue ;;
          esac
          lbl=$(findmnt -no LABEL --target "$d" 2>/dev/null || true)
          case "$lbl" in
            TENTAFLAKE_ENV | HERMES_ENV) printf '0\t%s\n' "$d" ;;
            *) printf '1\t%s\n' "$d" ;;
          esac
        done | sort -u | cut -f2-
      }

      # Fallback: mount an unmounted REMOVABLE partition ourselves. On a
      # headless agent host nothing auto-mounts, so this is usually THE path.
      # An unknown filesystem is untrusted input to the KERNEL, not just to us:
      # ro,nosuid,nodev,noexec. Never a fixed disk.
      # lsblk -J + jq, not -rno: raw mode collapses empty fields and shifts
      # LABEL into the MOUNTPOINT slot. jq is already a runtimeInput.
      # Appends to TF_MOUNTS in THIS shell — a subshell would lose the list the
      # cleanup trap has to unmount.
      usb_mount_removable() {
        command -v lsblk >/dev/null 2>&1 || return 0
        local dev lbl fs mp opts
        # LABEL goes LAST and is read last: tab is IFS *whitespace*, so `read`
        # collapses a run of them and an empty middle field would shift every
        # later field left. Trailing-empty is the only safe slot for it.
        while IFS=$'\t' read -r dev fs lbl; do
          [ -n "$dev" ] || continue
          case "$fs" in vfat | exfat | ext2 | ext3 | ext4 | ntfs | ntfs3) ;; *) continue ;; esac
          opts=ro,nosuid,nodev,noexec
          case "$fs" in vfat | exfat | ntfs | ntfs3) opts="$opts,uid=$(id -u)" ;; esac
          mp=$(mktemp -d); chmod 700 "$mp"
          if sudo mount -o "$opts" "$dev" "$mp" 2>/dev/null; then
            TF_MOUNTS+=("$mp")
            # Provenance the human can act on: "MY STICK /dev/sdb1", never the
            # mktemp basename they cannot tell two sticks apart by.
            TF_VOLNAME["$mp"]="''${lbl:+$lbl }$dev"
          else
            rmdir "$mp" 2>/dev/null || true
          fi
        done < <(lsblk -J -o PATH,LABEL,MOUNTPOINT,RM,HOTPLUG,FSTYPE 2>/dev/null | jq -r '
          [.blockdevices[] | recurse(.children[]?)] | .[]
          | select(.fstype != null and .fstype != "swap")
          | select(.rm == true or .hotplug == true)
          | select((.mountpoint // .mountpoints[0]? // "") == "")
          | [.path, .fstype, (.label // "")] | @tsv' 2>/dev/null || true)
      }

      # $1=keyvar. Fills the global TF_CAND with "value<TAB>provenance" lines.
      # Global, not stdout: a $(...) capture would run the mounting in a
      # subshell and strand the mounts past cleanup.
      # ponytail: two globs deep, .env/.txt/.key only, 40 hits max — predictable
      # and instant. A full-disk crawl is the upgrade path if people really bury
      # the key. Globs, not `find`: findutils is not in runtimeInputs.
      # Filenames and volume labels come off a stranger's stick. An ESC byte in
      # one rewrites the masked-key preview and the provenance line the user
      # relies on to decide whether to accept the key — same class as git's
      # core.quotePath.
      tf_plain() { LC_ALL=C tr -d '\000-\037\177'; }

      # $1=root $2=keyvar. Appends to TF_CAND.
      usb_scan_dir() {
        local root="$1" f v vol
        [ -n "$root" ] || return 0
        # Name the human can act on, best first: the label we mounted it under,
        # the volume label, the device node. The mktemp basename of a self-mount
        # tells nobody which of two sticks this is.
        vol=''${TF_VOLNAME[$root]-}
        [ -n "$vol" ] || vol=$(findmnt -no LABEL --target "$root" 2>/dev/null) || vol=""
        [ -n "$vol" ] || vol=$(basename "$(findmnt -no SOURCE --target "$root" 2>/dev/null || echo "$root")")
        [ -n "$vol" ] || vol=$(basename "$root")
        vol=$(printf '%s' "$vol" | tf_plain)
        for f in "$root"/*.env "$root"/.env "$root"/*.txt "$root"/*.key \
          "$root"/*/*.env "$root"/*/.env "$root"/*/*.txt "$root"/*/*.key; do
          [ -f "$f" ] && [ -r "$f" ] || continue
          # ponytail: a <=512-char key never lives in a >64K file. Without this
          # bound bare_value pulls a 400MB single-line blob into a shell
          # variable and the wizard just looks hung. 64K, not 4K: env_value
          # legitimately greps one var out of a multi-var .env.
          [ "$(stat -c%s "$f")" -le 65536 ] || continue
          # Skip binaries: a .key holding DER/PKCS#12 (or a key pasted into
          # Notepad and saved as UTF-16) otherwise reaches the null-byte read in
          # bare_value and prints a bash `warning:` into the pinned TUI.
          LC_ALL=C grep -qI "" "$f" 2>/dev/null || continue
          [ "''${#TF_CAND[@]}" -lt 40 ] || return 0
          v=$(env_value "$f" "$2") || v=$(bare_value "$f") || continue
          TF_CAND+=("$v"$'\t'"$(basename "$f" | tf_plain) (on $vol)")
        done
      }

      usb_candidates() {
        local root
        TF_CAND=()
        mapfile -t TF_ROOTS < <(usb_scan_roots) # mapfile: volume names have spaces
        for root in "''${TF_ROOTS[@]-}"; do usb_scan_dir "$root" "''${1:-}"; done
        # Only now pay for a privileged mount: gate on finding no KEY, not on
        # finding no directory. A stale /mnt/backup must not shadow the stick.
        if [ "''${#TF_CAND[@]}" -eq 0 ]; then
          usb_mount_removable
          for root in "''${TF_MOUNTS[@]-}"; do usb_scan_dir "$root" "''${1:-}"; done
        fi
      }

      cmd_agent_list() {
        agents_json_ensure
        if [ "$(jq '[.hermes[],.zeroclaw[]] | length' "$AGENTS_JSON")" -eq 0 ]; then
          echo "  no agents configured — add one with: ''${bold}tentaflake agent add''${reset}"
          return
        fi
        printf '%b  %-9s %-16s %-11s %-26s %-6s %-6s%b\n' \
          "$bold" RUNTIME NAME PROVIDER MODEL HOST SERVE "$reset"
        jq -r '
          (.hermes[]   | [ "hermes",   .name, .provider, .model, "-", "-" ]),
          (.zeroclaw[] | [ "zeroclaw", .name, .provider, .model, (.hostPort|tostring), (.servePort|tostring) ])
          | @tsv
        ' "$AGENTS_JSON" | while IFS=$'\t' read -r rt name prov model host serve; do
          printf '  %-9s %-16s %-11s %-26s %-6s %-6s\n' "$rt" "$name" "$prov" "$model" "$host" "$serve"
        done
      }

      # Presents whatever is on plugged-in media and sets `apikey` if the user
      # takes one. Reads $keyvar/$apikey from its caller via dynamic scope, and
      # is called plainly — a $(...) capture would run the mounting in a
      # subshell and strand the mounts past cleanup.
      key_from_media() {
        local labels=() choice i room
        # Only offer discovery to a human: a piped run must fall straight
        # through to the read path. No keyvar (zeroclaw+custom) still scans —
        # bare_value needs no var name at all.
        [ -t 0 ] || return 0
        usb_candidates "''${keyvar:-}"
        if [ "''${#TF_CAND[@]}" -eq 1 ]; then
          printf '%sFound a key on connected media:%s\n' "$green" "$reset" >&2
          printf '  %s%s\n  %s%s%s\n' \
            "$(mask_key "''${TF_CAND[0]%%$'\t'*}")" "$(key_hint "''${TF_CAND[0]%%$'\t'*}")" \
            "$dim" "''${TF_CAND[0]#*$'\t'}" "$reset" >&2
          # No default arg: ask() would render its own "[y]" on top of the
          # [Y/n] hint. Empty (just Enter) means yes.
          case "$(ask "use it? [Y/n]")" in
            n | N | no | NO) : ;;
            *) apikey=''${TF_CAND[0]%%$'\t'*} ;;
          esac
        elif [ "''${#TF_CAND[@]}" -gt 1 ]; then
          printf '%sFound %d keys on connected media:%s\n' "$green" "''${#TF_CAND[@]}" "$reset" >&2
          room=$(cand_room "''${HDR_LINES:-24}" "$HDR_ROWS")
          for i in "''${!TF_CAND[@]}"; do
            [ "''${#labels[@]}" -lt "$room" ] || break
            labels+=("$(mask_key "''${TF_CAND[i]%%$'\t'*}")  ←  ''${TF_CAND[i]#*$'\t'}")
          done
          [ "''${#labels[@]}" -eq "''${#TF_CAND[@]}" ] || printf '%s  … %d more found, not shown (small terminal)%s\n' \
            "$dim" "$(( ''${#TF_CAND[@]} - ''${#labels[@]} ))" "$reset" >&2
          labels+=("type or paste it myself")
          choice=$(pick "use which key?" "''${labels[@]}")
          for i in "''${!TF_CAND[@]}"; do
            [ "$choice" = "''${labels[i]-}" ] && { apikey=''${TF_CAND[i]%%$'\t'*}; break; }
          done
        fi
        TF_CAND=()
        tf_umount_all # done reading; don't hold the stick for the rest
        # Discoverability: the escape hatch is invisible to exactly the user who
        # needs it — the one whose stick is still in their bag.
        [ -n "$apikey" ] || printf '%sno key found on connected media — plug a stick in, then type r and press Enter to rescan%s\n' \
          "$dim" "$reset" >&2
        return 0
      }

      cmd_agent_add() {
        agents_json_ensure
        local runtime name provider model base_url="" host="" serve="" keyvar keyline secret_file entry answer

        # Get the sudo password prompt out of the way BEFORE the scroll region
        # exists and before the USB scan needs mount(8).
        sudo -v || true
        hdr_init

        runtime=$(pick "runtime" hermes zeroclaw)
        [ -n "$runtime" ] || { echo "''${red}error:''${reset} cancelled" >&2; exit 1; }

        while :; do
          name=$(ask "agent name (lowercase, digits, dashes)")
          if ! [[ "$name" =~ ^[a-z0-9-]+$ ]]; then
            echo "''${red}error:''${reset} name must match ^[a-z0-9-]+\$" >&2; continue
          fi
          if aj_has "$AGENTS_JSON" "$name"; then
            echo "''${red}error:''${reset} an agent named '$name' already exists" >&2; continue
          fi
          break
        done

        provider=$(pick "provider" openrouter anthropic openai custom)
        [ -n "$provider" ] || { echo "''${red}error:''${reset} cancelled" >&2; exit 1; }

        model=$(ask "model id (concrete id, e.g. anthropic/claude-sonnet-4)")
        [ -n "$model" ] || { echo "''${red}error:''${reset} model id is required" >&2; exit 1; }

        if [ "$provider" = "custom" ]; then
          base_url=$(ask "base_url (e.g. https://host/v1)")
        fi

        if [ "$runtime" = "zeroclaw" ]; then
          while :; do host=$(ask "hostPort"); [[ "$host" =~ ^[0-9]+$ ]] && break; echo "''${red}must be an integer''${reset}" >&2; done
          while :; do serve=$(ask "servePort"); [[ "$serve" =~ ^[0-9]+$ ]] && break; echo "''${red}must be an integer''${reset}" >&2; done
        fi

        # Env var the key is written under. zeroclaw expects a nested config key;
        # hermes/openai-style providers expect a single bearer var.
        if [ "$provider" = "custom" ] && [ "$runtime" != "zeroclaw" ]; then
          keyvar=$(ask "env var name for the API key" "OPENROUTER_API_KEY")
        else
          keyvar=$(provider_key_var "$provider")
        fi

        secret_file="$SECRET_BASE/$runtime-$name.env"

        # Key: imported from plugged-in media if we can find one, otherwise a
        # silent read. Either way it stays in this shell — never echoed
        # unmasked, never in argv/history, never in git or the Nix store.
        local apikey="" ok
        key_from_media

        # Typed/pasted — also the path when nothing is plugged in or the user
        # declined. The masked echo-back is the point: `read -rs` gives zero
        # feedback, so a paste that lost half the key looks identical to one
        # that worked.
        while [ -z "$apikey" ]; do
          printf 'API key for %s (hidden — paste is fine, blank aborts): ' "$name" >&2
          read -rs apikey || true
          printf '\n' >&2
          [ -n "$apikey" ] || { echo "''${red}error:''${reset} no API key entered — aborting" >&2; exit 1; }
          # `r` = "the stick is in my bag" — rescan without losing every answer.
          case "$apikey" in r | R) apikey=""; key_from_media; continue ;; esac
          if ! sane_key "$apikey"; then
            echo "''${red}that is not a usable key''${reset} — it contains spaces or control characters" >&2
            apikey=""; continue
          fi
          if ! plausible_key "$apikey"; then
            # ponytail: a heuristic, not a gate. A custom/self-hosted gateway
            # key can be anything (LiteLLM ships `sk-1234`). Discovery keeps the
            # strict form on purpose — nothing there has a human to confirm it.
            if [ "''${#apikey}" -lt 16 ] || [ "''${#apikey}" -gt 512 ]; then
              ok="it is ''${#apikey} characters long"
            else
              ok="it contains a character API keys normally don't"
            fi
            printf '  %sunusual API key%s — %s. Use it anyway? [y/N] ' "$yellow" "$reset" "$ok" >&2
            read -r ok || true
            case "$ok" in y | Y | yes | YES) ;; *) apikey=""; continue ;; esac
          fi
          printf '  got %s%s%s%s — correct? [Y/n] ' "$bold" "$(mask_key "$apikey")" "$reset" "$(key_hint "$apikey")" >&2
          read -r ok || true
          case "$ok" in n | N | no | NO) apikey="" ;; esac
        done

        if [ "$runtime" = "zeroclaw" ]; then
          keyline="ZEROCLAW_providers__models__''${provider}__default__api_key=$apikey"
        else
          keyline="$keyvar=$apikey"
        fi

        # Stage to a 0600 temp owned by us, then let root install it. The key
        # goes via a file, so it never appears in any process's argv.
        # File-scope, not local: the cleanup trap owns it, so a Ctrl-C at the
        # sudo password prompt cannot leave the plaintext key in /tmp.
        TF_TMP=$(mktemp)
        chmod 600 "$TF_TMP"
        printf '%s\n' "$keyline" > "$TF_TMP"
        unset apikey keyline
        if ! sudo install -D -m 600 -o root -g root "$TF_TMP" "$secret_file"; then
          rm -f "$TF_TMP"; TF_TMP=""
          echo "''${red}error:''${reset} failed to write secret to $secret_file" >&2
          exit 1
        fi
        rm -f "$TF_TMP"; TF_TMP=""

        if [ "$runtime" = "hermes" ]; then
          entry=$(jq -n --arg name "$name" --arg provider "$provider" --arg model "$model" \
            --arg base_url "$base_url" --arg envFile "$secret_file" \
            '{name:$name, provider:$provider, model:$model,
              base_url: (if $base_url=="" then null else $base_url end), envFile:$envFile}')
        else
          entry=$(jq -n --arg name "$name" --arg provider "$provider" --arg model "$model" \
            --arg base_url "$base_url" --arg envFile "$secret_file" \
            --argjson hostPort "$host" --argjson servePort "$serve" \
            '{name:$name, provider:$provider, model:$model,
              base_url: (if $base_url=="" then null else $base_url end),
              hostPort:$hostPort, servePort:$servePort, envFile:$envFile}')
        fi
        aj_append "$AGENTS_JSON" "$runtime" "$entry"
        git -C "$FLAKE_DIR" add agents.json >/dev/null 2>&1 || true

        echo "''${green}✓''${reset} added $runtime agent '$name' → $AGENTS_JSON"
        echo "  secret: $secret_file (root:root 0600, key not in git)"
        echo "next: rebuild so the container is created from this config."
        printf 'Rebuild now? [y/N] '
        read -r answer || true
        # Drop the scroll region first: nixos-rebuild's output should not be
        # squeezed through the few rows left under the pinned logo.
        tf_cleanup
        case "$answer" in y | Y | yes | YES) cmd_rebuild ;; *) echo "Apply later with: tentaflake rebuild" ;; esac
      }

      cmd_agent_set_model() {
        agents_json_ensure
        local name="''${1:-}" cur model
        [ -n "$name" ] || { echo "''${red}error:''${reset} usage: tentaflake agent set-model <name>" >&2; exit 2; }
        if ! aj_has "$AGENTS_JSON" "$name"; then
          echo "''${red}error:''${reset} no agent '$name' in $AGENTS_JSON" >&2; exit 2
        fi
        cur=$(aj_get "$AGENTS_JSON" "$name" model)
        echo "current model for '$name': $cur"
        model=$(ask "new model id" "$cur")
        [ -n "$model" ] || { echo "''${red}error:''${reset} model id is required" >&2; exit 1; }
        aj_set_model "$AGENTS_JSON" "$name" "$model"
        git -C "$FLAKE_DIR" add agents.json >/dev/null 2>&1 || true
        echo "''${green}✓''${reset} set model for '$name' → $model"
        echo "apply with: tentaflake rebuild"
      }

      cmd_agent_remove() {
        agents_json_ensure
        local name="''${1:-}" runtime envfile answer
        [ -n "$name" ] || { echo "''${red}error:''${reset} usage: tentaflake agent remove <name>" >&2; exit 2; }
        if ! aj_has "$AGENTS_JSON" "$name"; then
          echo "''${red}error:''${reset} no agent '$name' in $AGENTS_JSON" >&2; exit 2
        fi
        runtime=$(aj_runtime "$AGENTS_JSON" "$name")
        envfile=$(aj_get "$AGENTS_JSON" "$name" envFile)
        printf 'Remove %s agent %s from agents.json? [y/N] ' "$runtime" "$name"
        read -r answer || true
        case "$answer" in y | Y | yes | YES) ;; *) echo "cancelled."; exit 0 ;; esac
        aj_remove "$AGENTS_JSON" "$name"
        git -C "$FLAKE_DIR" add agents.json >/dev/null 2>&1 || true
        echo "''${green}✓''${reset} removed '$name' from $AGENTS_JSON"
        if [ -n "$envfile" ] && [ "$envfile" != "null" ]; then
          printf 'Also delete its secret file %s? [y/N] ' "$envfile"
          read -r answer || true
          case "$answer" in
            y | Y | yes | YES) sudo rm -f "$envfile" && echo "deleted $envfile" ;;
            *) echo "kept $envfile" ;;
          esac
        fi
        echo "apply with: tentaflake rebuild"
      }

      # Hidden smoke test for the aj_* jq primitives (see block comment above).
      cmd_agent_selftest() {
        local f; f=$(mktemp)
        printf '%s\n' '{"hermes":[],"zeroclaw":[]}' > "$f"
        aj_append "$f" hermes   '{"name":"t1","provider":"openrouter","model":"m1","base_url":null,"envFile":"/x"}'
        aj_append "$f" zeroclaw '{"name":"t2","provider":"openai","model":"m2","base_url":null,"hostPort":1,"servePort":2,"envFile":"/y"}'
        aj_has "$f" t1 || { echo "FAIL: append/has"; rm -f "$f"; exit 1; }
        aj_set_model "$f" t1 m1b
        [ "$(aj_get "$f" t1 model)" = "m1b" ] || { echo "FAIL: set-model"; rm -f "$f"; exit 1; }
        [ "$(aj_runtime "$f" t2)" = "zeroclaw" ] || { echo "FAIL: runtime"; rm -f "$f"; exit 1; }
        aj_remove "$f" t1
        aj_has "$f" t1 && { echo "FAIL: remove"; rm -f "$f"; exit 1; }
        [ "$(aj_get "$f" t2 name)" = "t2" ] || { echo "FAIL: t2 clobbered"; rm -f "$f"; exit 1; }

        # ── key parsing: parse, NEVER execute ──
        local d; d=$(mktemp -d)
        printf 'X=1\n# comment\nexport OPENROUTER_API_KEY="sk-or-v1-abcdefghijklmnop"\r\n' > "$d/a.env" # gitleaks:allow (fake fixture key for env_value test)
        printf 'sk-ant-api03-ZZZZZZZZZZZZZZZZZZZZ\n' > "$d/b.key"
        # shellcheck disable=SC2016  # the $(...) MUST stay literal — that is the fixture
        printf 'OPENROUTER_API_KEY=$(touch %s/pwned)aaaaaaaaaaaaaaaa\n' "$d" > "$d/c.env"
        printf 'OPENROUTER_API_KEY=\n' > "$d/e.env"
        printf 'OPENROUTER_API_KEY=short\n' > "$d/f.env"
        printf 'OPENROUTER_API_KEY=has spaces and is quite long\n' > "$d/g.env"
        [ "$(env_value "$d/a.env" OPENROUTER_API_KEY)" = "sk-or-v1-abcdefghijklmnop" ] || { echo "FAIL: env_value"; rm -rf "$d" "$f"; exit 1; }
        env_value "$d/a.env" ANTHROPIC_API_KEY && { echo "FAIL: matched the wrong var"; rm -rf "$d" "$f"; exit 1; }
        [ "$(bare_value "$d/b.key")" = "sk-ant-api03-ZZZZZZZZZZZZZZZZZZZZ" ] || { echo "FAIL: bare_value"; rm -rf "$d" "$f"; exit 1; }
        bare_value "$d/e.env" && { echo "FAIL: bare_value took an empty env line as a token"; rm -rf "$d" "$f"; exit 1; }
        env_value "$d/f.env" OPENROUTER_API_KEY && { echo "FAIL: accepted a 5-char value"; rm -rf "$d" "$f"; exit 1; }
        env_value "$d/g.env" OPENROUTER_API_KEY && { echo "FAIL: accepted a value with spaces"; rm -rf "$d" "$f"; exit 1; }
        env_value "$d/c.env" OPENROUTER_API_KEY >/dev/null 2>&1 || true
        [ ! -e "$d/pwned" ] || { echo "FAIL: USB file content was EXECUTED"; rm -rf "$d" "$f"; exit 1; }
        [ "$(mask_key sk-or-v1-abcdefghijklmnop)" = "sk-or-…mnop (25 chars)" ] || { echo "FAIL: mask_key"; rm -rf "$d" "$f"; exit 1; }
        [ "$(key_hint sk-or-v1-x)" = " · looks like an OpenRouter key" ] || { echo "FAIL: key_hint"; rm -rf "$d" "$f"; exit 1; }
        # strict for media / advisory for the typed path — must not silently merge
        plausible_key "sk-1234" && { echo "FAIL: plausible_key took a short token"; rm -rf "$d" "$f"; exit 1; }
        plausible_key 'p@ssw0rd-Very-Long-Token!' && { echo "FAIL: plausible_key took an odd charset"; rm -rf "$d" "$f"; exit 1; }
        sane_key "sk-1234" || { echo "FAIL: sane_key rejected a short typed key"; rm -rf "$d" "$f"; exit 1; }
        sane_key 'p@ssw0rd-Very-Long-Token!' || { echo "FAIL: sane_key rejected a gateway token"; rm -rf "$d" "$f"; exit 1; }
        sane_key "has spaces" && { echo "FAIL: sane_key took spaces"; rm -rf "$d" "$f"; exit 1; }
        sane_key "$(printf 'ab\tcd')" && { echo "FAIL: sane_key took a control char"; rm -rf "$d" "$f"; exit 1; }

        # lsblk field order: tab is IFS *whitespace*, so `read` collapses a run
        # of them. An unlabelled device must not shift fstype out of its slot.
        local _ld lfs llbl
        IFS=$'\t' read -r _ld lfs llbl < <(printf '/dev/sdc1\tvfat\t\n')
        [ "$lfs" = vfat ] && [ -z "$llbl" ] || { echo "FAIL: empty LABEL collapsed the lsblk fields"; rm -rf "$d" "$f"; exit 1; }
        IFS=$'\t' read -r _ld lfs llbl < <(printf '/dev/sdb1\tvfat\tMY STICK\n')
        [ "$lfs" = vfat ] && [ "$llbl" = "MY STICK" ] || { echo "FAIL: lsblk LABEL with a space"; rm -rf "$d" "$f"; exit 1; }

        # ── scan hygiene: huge/binary files must not be read or reported ──
        local sd; sd=$(mktemp -d)
        printf 'sk-or-v1-goodgoodgoodgood\n' > "$sd/good.txt"
        head -c 200000 /dev/zero | tr '\0' 'a' > "$sd/big.txt"
        head -c 300 /dev/urandom > "$sd/license.key"
        printf 'sk-or-v1-esc\033injected\n' > "$sd/$(printf 'e\033vil').txt"
        TF_CAND=(); usb_scan_dir "$sd" OPENROUTER_API_KEY
        [ "''${#TF_CAND[@]}" -ge 1 ] || { echo "FAIL: usb_scan_dir found nothing"; rm -rf "$sd" "$d" "$f"; exit 1; }
        printf '%s\n' "''${TF_CAND[@]}" | grep -q "$(printf '\033')" && { echo "FAIL: ESC byte survived into provenance"; rm -rf "$sd" "$d" "$f"; exit 1; }
        printf '%s\n' "''${TF_CAND[@]}" | grep -q 'big\.txt' && { echo "FAIL: scanned an oversized file"; rm -rf "$sd" "$d" "$f"; exit 1; }
        printf '%s\n' "''${TF_CAND[@]}" | grep -q 'license\.key' && { echo "FAIL: took a binary file as a key"; rm -rf "$sd" "$d" "$f"; exit 1; }
        TF_CAND=(); rm -rf "$sd"
        rm -rf "$d"

        # ── header geometry + degradation + teardown ──
        [ "$(hdr_plan 40 100)" = 15 ] || { echo "FAIL: hdr_plan full"; rm -f "$f"; exit 1; }
        [ "$(hdr_plan 18 80)" = 2 ] || { echo "FAIL: hdr_plan compact"; rm -f "$f"; exit 1; }
        [ "$(hdr_plan 10 30)" = 0 ] || { echo "FAIL: hdr_plan degrade"; rm -f "$f"; exit 1; }
        [ "$(TERM=linux hdr_plan 40 100)" = 2 ] || { echo "FAIL: hdr_plan VT has no braille glyphs"; rm -f "$f"; exit 1; }
        [ "$(cand_room 24 15)" = 5 ] || { echo "FAIL: cand_room 24x80"; rm -f "$f"; exit 1; }
        [ "$(cand_room 40 15)" = 21 ] || { echo "FAIL: cand_room large"; rm -f "$f"; exit 1; }
        [ "$(cand_room 12 15)" = 3 ] || { echo "FAIL: cand_room floor"; rm -f "$f"; exit 1; }
        ( NO_COLOR=1; hdr_capable ) && { echo "FAIL: hdr_capable ignored NO_COLOR"; rm -f "$f"; exit 1; }
        ( TERM=dumb; hdr_capable ) && { echo "FAIL: hdr_capable ignored TERM=dumb"; rm -f "$f"; exit 1; }
        # teardown must emit the scroll-region reset, then be a no-op
        local t; t=$(mktemp); exec 3>"$t"; HDR_ROWS=4; TF_CLEANED=""
        TF_TMP=$(mktemp) # the plaintext-key staging file must not survive
        local staged="$TF_TMP"
        tf_cleanup; exec 3>/dev/null
        [ ! -e "$staged" ] || { echo "FAIL: tf_cleanup left the staged key on disk"; rm -f "$f" "$t" "$staged"; exit 1; }
        grep -q "$(printf '\033')\[r" "$t" || { echo "FAIL: tf_cleanup did not reset the scroll region"; rm -f "$f" "$t"; exit 1; }
        [ "$HDR_ROWS" -eq 0 ] || { echo "FAIL: tf_cleanup not idempotent"; rm -f "$f" "$t"; exit 1; }
        TF_CLEANED=""
        rm -f "$t"

        rm -f "$f"
        echo "''${green}agent selftest OK''${reset}"
      }

      cmd_agent() {
        local action="''${1:-list}"; shift || true
        case "$action" in
          list | ls | "") cmd_agent_list ;;
          add) cmd_agent_add ;;
          set-model) cmd_agent_set_model "''${1:-}" ;;
          remove | rm) cmd_agent_remove "''${1:-}" ;;
          __selftest) cmd_agent_selftest ;;
          *)
            echo "''${red}error:''${reset} unknown agent command '$action'" >&2
            echo "usage: tentaflake agent [list | add | set-model <name> | remove <name>]" >&2
            exit 2
            ;;
        esac
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
            if ! command -v tentaflake-top >/dev/null 2>&1; then
              echo "''${red}error:''${reset} tentaflake-top not installed — enable the audit daemon:" >&2
              echo "  tentaflake.auditd.enable = true;" >&2
              exit 1
            fi
            exec tentaflake-top "$@"
            ;;
          backup)
            require_name "''${2:-}"
            cmd_backup "$2"
            ;;
          doctor) cmd_doctor ;;
          console) cmd_console ;;
          agent)
            shift || true
            cmd_agent "$@"
            ;;
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
