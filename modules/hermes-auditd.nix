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
  # admin reads the audit DB (and runs `hermes top`) without sudo.
  auditUser = "hermes-audit";
  auditGroup = "hermes-audit";

  # Auto-discover agent state dirs from the generated OCI containers when the
  # operator hasn't set watchDirs explicitly. mkHermesAgent names each container
  # `hermes-<name>` with state dir `/var/lib/hermes-<name>`, so we map the former
  # to the latter. Agents with a custom stateDir must list watchDirs by hand.
  discoveredDirs = lib.mapAttrsToList (name: _: "/var/lib/${name}") (
    lib.filterAttrs (
      name: _: lib.hasPrefix "hermes-" name
    ) config.virtualisation.oci-containers.containers
  );
  effectiveWatchDirs = if cfg.watchDirs != [ ] then cfg.watchDirs else discoveredDirs;
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
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ auditPkg ];

    # ── Identity ──
    users.groups.${auditGroup} = { };
    users.users.${auditUser} = {
      isSystemUser = true;
      group = auditGroup;
      description = "Hermes audit daemon";
    };

    # ── Let the admin read the audit DB (and run `hermes top`) without sudo ──
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
  };
}
