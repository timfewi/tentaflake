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

  agentContainers = config.virtualisation.oci-containers.containers;
  agentRecords = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (
      container: _:
      let
        runtime =
          if lib.hasPrefix "zeroclaw-" container then
            "zeroclaw"
          else if lib.hasPrefix "hermes-" container then
            "hermes"
          else
            "agent";
        name = lib.removePrefix "${runtime}-" container;
      in
      "${runtime}\t${name}\t${container}\t${backend}-${container}.service"
    ) agentContainers
  );

  # ── tentaflake: operator CLI for all declarative agent containers ──
  tentaflakeCli = pkgs.writeShellApplication {
    name = "tentaflake";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.gawk
      pkgs.systemd
      statusBanner
    ];
    text = ''
      # Container backend is fixed at build time from tentaflake.containerBackend.
      BACKEND="${backend}"

      bold=$(printf '\033[1m'); reset=$(printf '\033[0m'); red=$(printf '\033[31m')

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
        tentaflake help                Show this help
      EOF
      }

      agent_names() {
        agent_records | awk -F '\t' 'NF == 4 { print $2 }'
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

      kv() { printf '  %b%-10s%b %s\n' "$dim" "$1" "$reset" "$2"; }

      # ── Header ──
      printf '\n%b  ╔═╗ tentaflake%b  %b%s%b\n' "$cyan" "$reset" "$bold" "$(hostname)" "$reset"
      printf '%b  ╚═╝ multi-runtime agent host%b\n\n' "$dim" "$reset"

      # ── System facts ──
      kv "kernel" "$(uname -sr)"
      up=$(uptime -p 2>/dev/null | sed 's/^up //' || true)
      if [ -z "$up" ]; then up=$(uptime 2>/dev/null || true); fi
      kv "uptime" "$up"
      kv "load"   "$(awk '{print $1", "$2", "$3}' /proc/loadavg 2>/dev/null || true)"

      mem=$(free -h 2>/dev/null | awk '/^Mem:/ {print $3" / "$2}' || true)
      [ -n "$mem" ] && kv "memory" "$mem"

      disk=$(df -h / 2>/dev/null | awk 'NR==2 {print $3" / "$2" ("$5" used)"}' || true)
      [ -n "$disk" ] && kv "disk /" "$disk"

      if command -v tailscale >/dev/null 2>&1; then
        ts=$(tailscale ip -4 2>/dev/null | head -n1 || true)
        if [ -n "$ts" ]; then
          kv "tailnet" "$ts"
        else
          kv "tailnet" "''${yellow}not connected (sudo tailscale up)''${reset}"
        fi
      fi

      # ── Agents ──
      records=${lib.escapeShellArg agentRecords}

      printf '\n  %b%s%b\n' "$bold" "AGENTS" "$reset"
      if [ -z "$records" ]; then
        printf '    %bnone defined — see my-agents.nix.example%b\n' "$dim" "$reset"
      else
        while IFS=$'\t' read -r runtime n container unit; do
          [ -n "$container" ] || continue
          st=$(systemctl is-active "$unit" 2>/dev/null || true)
          case "$st" in
            active)
              dot="●"
              if [ "$runtime" = "zeroclaw" ]; then
                icon_color=$blue
                status_color=$blue
              else
                icon_color=$yellow
                status_color=$yellow
              fi
              ;;
            failed)
              dot="●"
              icon_color=$red
              status_color=$red
              ;;
            *)
              dot="○"
              icon_color=$yellow
              status_color=$dim
              ;;
          esac
          printf '    %b%s%b %-20s %-10s %b%s%b\n' \
            "$icon_color" "$dot" "$reset" "$n" "$runtime" "$status_color" "$st" "$reset"
        done <<< "$records"
      fi

      printf '\n  %brun %b%stentaflake%b%b to manage agents · %btentaflake help%b for commands%b\n\n' \
        "$dim" "$reset" "$bold" "$reset" "$dim" "$bold" "$reset" "$reset"
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
    rebuild = "sudo nixos-rebuild switch --flake /etc/nixos#${hostName}";
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
