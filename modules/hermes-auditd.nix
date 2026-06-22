{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.tentaflake.hermes-auditd;
  auditPkg = pkgs.callPackage ../pkgs/hermes-auditd { };
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

    systemd.services.hermes-auditd = {
      description = "Hermes agent filesystem audit daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "local-fs.target" ];

      serviceConfig = {
        ExecStart = "${auditPkg}/bin/hermes-auditd";
        Restart = "on-failure";
        RestartSec = "5s";
        Type = "simple";

        Environment = [
          "AUDIT_PORT=${toString cfg.port}"
          "AUDIT_DB_PATH=${cfg.dbPath}"
          "AUDIT_WATCH_DIRS=${lib.concatStringsSep "," cfg.watchDirs}"
          "AUDIT_RETENTION_HOURS=${toString cfg.retentionHours}"
        ];

        # Hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        CapabilityBoundingSet = [ "" ];
        SystemCallFilter = [ "@system-service" ];
        LockPersonality = true;

        # State directory for SQLite DB
        StateDirectory = "hermes-audit";
        StateDirectoryMode = "0700";
      };
    };
  };
}
