{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.tentaflake.hermes-auditd;
  auditPkg = pkgs.callPackage ../pkgs/hermes-auditd { };

  # Dedicated unprivileged identity for the daemon and a group through which the
  # admin reads the audit DB (and runs `tentaflake top`) without sudo.
  auditUser = "hermes-audit";
  auditGroup = "hermes-audit";

  # Auto-discover the conventional state directory of every declarative AGENT
  # container (hermes-*/zeroclaw-*) — only agent prefixes, so an unrelated
  # oci-container never gets its /var/lib dir watched or console-exposed.
  # Agents with a custom stateDir must list watchDirs by hand.
  discoveredDirs = lib.mapAttrsToList (name: _: "/var/lib/${name}") (
    lib.filterAttrs (
      name: _: lib.hasPrefix "hermes-" name || lib.hasPrefix "zeroclaw-" name
    ) config.virtualisation.oci-containers.containers
  );
  effectiveWatchDirs = if cfg.watchDirs != [ ] then cfg.watchDirs else discoveredDirs;

  # Console roots: when the operator sets none, derive one root per watched agent
  # state dir. Hermes dirs keep their historical bare label ("coding"); other
  # runtimes keep the prefix ("zeroclaw-assistant") so runtimes stay apart —
  # matching how the daemon labels events (see watcher.agentNameFromPath).
  derivedConsoleRoots = map (dir: {
    name = lib.removePrefix "/var/lib/" (lib.removePrefix "/var/lib/hermes-" dir);
    path = dir;
  }) effectiveWatchDirs;
  # `roots` (when set) overrides the auto-derived agent-home list; `extraRoots`
  # is always appended, so operators can add data-disk mounts while keeping the
  # auto-derived homes. Roots whose path is missing are skipped at runtime.
  baseConsoleRoots = if cfg.console.roots != [ ] then cfg.console.roots else derivedConsoleRoots;
  effectiveConsoleRoots = baseConsoleRoots ++ cfg.console.extraRoots;
  consoleRootsEnv = lib.concatMapStringsSep "," (r: "${r.name}:${r.path}") effectiveConsoleRoots;

  # A single explorable root: a display name and the host dir it exposes.
  rootType = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Display name (top-level folder) in the explorer.";
      };
      path = lib.mkOption {
        type = lib.types.str;
        description = "Host directory exposed read-only under this name.";
      };
    };
  };
in
{
  options.tentaflake.hermes-auditd = {
    enable = lib.mkEnableOption "Hermes audit daemon (filesystem change monitoring)";

    watchDirs = lib.mkOption {
      type = with lib.types; listOf str;
      default = [ ];
      example = [
        "/var/lib/hermes-coding"
        "/var/lib/hermes-research"
      ];
      description = "Directories to monitor for filesystem changes";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 9090;
      description = "HTTP/WebSocket listen port";
    };

    dbPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/hermes-audit/events.db";
      description = "Path to the SQLite database";
    };

    retentionHours = lib.mkOption {
      type = lib.types.int;
      default = 24;
      description = "Event retention window in hours (older events are pruned)";
    };

    # ── Agent Console: read-only web file explorer + live activity monitor ──
    console = {
      enable = lib.mkEnableOption ''
        the Hermes Agent Console: a read-only web UI combining a file explorer
        over the agent state dirs with a live activity monitor (backed by the
        same events.db). Bind it to loopback and publish on the tailnet via
        `tailscale serve`'';

      addr = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1:9090";
        description = ''
          Loopback listen address for the console. The daemon itself opens no
          socket, so reusing :9090 here is safe.
        '';
      };

      roots = lib.mkOption {
        type = lib.types.listOf rootType;
        default = [ ];
        example = [
          {
            name = "my-agent";
            path = "/var/lib/hermes-my-agent";
          }
        ];
        description = ''
          File roots exposed by the explorer. When left empty, roots are
          auto-derived from the watched agent state dirs (one per
          `/var/lib/hermes-<name>`); set this only to override that list
          entirely. To keep the auto-derived homes and merely add more roots
          (e.g. data-disk mounts), use `extraRoots` instead. Secret files
          (.env, auth.json, config.yaml, keys, …) are always hidden regardless
          of root, at every depth.
        '';
      };

      extraRoots = lib.mkOption {
        type = lib.types.listOf rootType;
        default = [ ];
        example = [
          {
            name = "my-agent-data";
            path = "/srv/agent-data/my-agent";
          }
        ];
        description = ''
          Additional file roots appended to the (derived or explicit) `roots`
          list. Use this to expose data-disk mounts alongside the auto-derived
          agent homes without restating them. Roots whose path is missing at
          runtime are skipped with a warning.
        '';
      };

      extraDeny = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [
          "*.sqlite"
          "drafts"
        ];
        description = ''
          Extra basename glob patterns to hide, appended to the built-in
          secret/clutter denylist (case-insensitive).
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ auditPkg ];

    # The daemon adds one inotify watch per agent directory; raise the kernel cap.
    boot.kernel.sysctl."fs.inotify.max_user_watches" = 524288;

    # ── Identity ──
    users.groups.${auditGroup} = { };
    users.users.${auditUser} = {
      isSystemUser = true;
      group = auditGroup;
      description = "Hermes audit daemon";
    };

    # ── Let the admin read the audit DB (and run `tentaflake top`) without sudo ──
    # The DB lives in the daemon's StateDirectory (group hermes-audit, group-rw);
    # adding the admin to that group is the whole access grant.
    users.users.${config.tentaflake.adminUser}.extraGroups = lib.mkIf config.tentaflake.users.enable [
      auditGroup
    ];

    systemd.services.hermes-auditd = {
      description = "Hermes agent filesystem audit daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "local-fs.target" ];

      serviceConfig = {
        ExecStart = "${auditPkg}/bin/hermes-auditd";
        Restart = "on-failure";
        RestartSec = "5s";
        Type = "simple";

        User = auditUser;
        Group = auditGroup;

        Environment = [
          "AUDIT_PORT=${toString cfg.port}"
          "AUDIT_DB_PATH=${cfg.dbPath}"
          "AUDIT_WATCH_DIRS=${lib.concatStringsSep "," effectiveWatchDirs}"
          "AUDIT_RETENTION_HOURS=${toString cfg.retentionHours}"
        ];

        # Hardening. The daemon runs unprivileged; CAP_DAC_READ_SEARCH is the
        # one privilege it needs — a *read-only* DAC bypass to inotify-watch the
        # agents' 0700 state dirs (owned by their own hermes-<name> users). It
        # can read everywhere but write nowhere outside its StateDirectory.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        AmbientCapabilities = [ "CAP_DAC_READ_SEARCH" ];
        CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
        SystemCallFilter = [ "@system-service" ];
        LockPersonality = true;

        # SQLite DB dir: group-accessible so admins in the hermes-audit group can
        # read it (and open the WAL files) without sudo. UMask 0007 → files 0660.
        StateDirectory = "hermes-audit";
        StateDirectoryMode = "0770";
        UMask = "0007";
      };
    };

    # ── Agent Console web server (read-only) ──
    # Same unprivileged identity + read-only DAC bypass as the daemon: it reads
    # the agents' 0700 state dirs and the audit DB but can write nowhere outside
    # its (shared) StateDirectory. The HTTP surface is GET-only, bound to
    # loopback; expose it on the tailnet with `tailscale serve`.
    systemd.services.tentaflake-console = lib.mkIf cfg.console.enable {
      description = "Hermes Agent Console (read-only file explorer + live monitor)";
      wantedBy = [ "multi-user.target" ];
      after = [
        "local-fs.target"
        "hermes-auditd.service"
      ];

      serviceConfig = {
        ExecStart = "${auditPkg}/bin/tentaflake-console";
        Restart = "on-failure";
        RestartSec = "5s";
        Type = "simple";

        User = auditUser;
        Group = auditGroup;

        Environment = [
          "AUDIT_CONSOLE_ADDR=${cfg.console.addr}"
          "AUDIT_CONSOLE_ROOTS=${consoleRootsEnv}"
          "AUDIT_CONSOLE_DENY=${lib.concatStringsSep "," cfg.console.extraDeny}"
          "AUDIT_DB_PATH=${cfg.dbPath}"
          "AUDIT_RETENTION_HOURS=${toString cfg.retentionHours}"
        ];

        # Read-only DAC bypass to read the agents' 0700 dirs; no write privilege.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        AmbientCapabilities = [ "CAP_DAC_READ_SEARCH" ];
        CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
        SystemCallFilter = [ "@system-service" ];
        LockPersonality = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
        ];

        # Shares the audit DB dir with the daemon (read for queries; WAL sidecars).
        StateDirectory = "hermes-audit";
        StateDirectoryMode = "0770";
        UMask = "0007";
      };
    };
  };
}
