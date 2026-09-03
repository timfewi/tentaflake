{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.backup;
  safePath =
    root: path:
    lib.hasPrefix root path
    && lib.match "^/[A-Za-z0-9._+/-]+$" path != null
    && !(lib.elem ".." (lib.splitString "/" path))
    && !(lib.elem "." (lib.splitString "/" path))
    && !(lib.hasInfix "//" path);
  safeRuntimePath = safePath "/run/";
in
{
  options.tentaflake.backup = {
    enable = lib.mkEnableOption "encrypted Restic backup for agent and broker state";
    paths = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [
        "/var/lib/hermes-coding"
        "/var/lib/tentaflake-broker-llm-hermes-coding"
      ];
      description = "Explicit agent state, workspace, broker audit, and budget paths to back up.";
    };
    repositoryFile = lib.mkOption {
      type = lib.types.str;
      default = "";
      example = "/run/agenix/restic-repository";
      description = "Runtime-only file containing the Restic repository location.";
    };
    passwordFile = lib.mkOption {
      type = lib.types.str;
      default = "";
      example = "/run/agenix/restic-password";
      description = "Runtime-only Restic repository encryption password file.";
    };
    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/agenix/restic-environment";
      description = "Optional runtime-only backend credential environment file.";
    };
    initialize = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Initialize a missing repository. Keep false for established production repositories.";
    };
    timerConfig = lib.mkOption {
      type = lib.types.nullOr (lib.types.attrsOf lib.types.unspecified);
      default = {
        OnCalendar = "03:00";
        Persistent = true;
        RandomizedDelaySec = "30m";
      };
      description = "systemd timer configuration, or null for manual/test invocation.";
    };
    pruneOpts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "--keep-daily 7"
        "--keep-weekly 4"
        "--keep-monthly 6"
      ];
    };
    checkOpts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "--read-data-subset=5%" ];
      description = "Arguments for the post-backup Restic integrity check.";
    };
    lastSuccessMaxAgeHours = lib.mkOption {
      type = lib.types.ints.positive;
      default = 36;
      description = "Age after which the security doctor reports the last successful backup as stale.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.paths != [ ] && lib.all (safePath "/var/lib/") cfg.paths;
        message = "tentaflake backup requires explicit paths below /var/lib; broad roots and implicit discovery are rejected.";
      }
      {
        assertion = safeRuntimePath cfg.repositoryFile && safeRuntimePath cfg.passwordFile;
        message = "tentaflake backup repositoryFile and passwordFile must be safe runtime-only paths below /run.";
      }
      {
        assertion = cfg.environmentFile == null || safeRuntimePath cfg.environmentFile;
        message = "tentaflake backup environmentFile must be a safe runtime-only path below /run.";
      }
    ];

    services.restic.backups.tentaflake = {
      inherit (cfg)
        paths
        repositoryFile
        passwordFile
        environmentFile
        initialize
        timerConfig
        pruneOpts
        checkOpts
        ;
      runCheck = true;
      inhibitsSleep = true;
      extraBackupArgs = [ "--one-file-system" ];
    };

    systemd.services.restic-backups-tentaflake.unitConfig.OnSuccess = [
      "tentaflake-backup-success.service"
    ];

    systemd.services.tentaflake-backup-success = {
      description = "Record the last successful Tentaflake backup";
      serviceConfig = {
        Type = "oneshot";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        StateDirectory = "tentaflake-backup";
        StateDirectoryMode = "0750";
        RestrictAddressFamilies = [ "AF_UNIX" ];
        CapabilityBoundingSet = [ ];
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        SystemCallArchitectures = "native";
      };
      script = ''
        ${pkgs.coreutils}/bin/touch "$STATE_DIRECTORY/last-success"
      '';
    };
  };
}
