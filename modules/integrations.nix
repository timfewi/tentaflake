{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.integrations;
in
{
  options.tentaflake.integrations = {
    enable = lib.mkEnableOption "fixed-grant read-only enterprise connector front door";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-integrations { };
      description = "Package providing the authenticated connector front door.";
    };
    configFile = lib.mkOption {
      type = lib.types.str;
      default = "/run/tentaflake-integrations-config.json";
      description = "Root-owned runtime-only JSON configuration path; never credential contents in Nix.";
    };
    advisory.enable = lib.mkEnableOption "evidence-linked human review storage (no model runtime or remediation authority)";
  };
  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = lib.hasPrefix "/" cfg.configFile && !(lib.hasPrefix "/nix/store/" cfg.configFile);
        message = "Connector configuration must be an absolute runtime-only path outside the Nix store.";
      }
    ];
    users.groups.tentaflake-integrations = { };
    users.users.tentaflake-integrations = {
      isSystemUser = true;
      group = "tentaflake-integrations";
    };
    systemd.services.tentaflake-integrations = {
      description = "Tentaflake fixed-grant read-only connectors";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/tentaflake-integrations ${
          if cfg.advisory.enable then "serve-advisory" else "serve"
        } ${lib.escapeShellArg cfg.configFile}";
        User = "tentaflake-integrations";
        Group = "tentaflake-integrations";
        RuntimeDirectory = "tentaflake-connectors";
        RuntimeDirectoryMode = "0711";
        StateDirectory = "tentaflake-integrations";
        StateDirectoryMode = "0700";
        UMask = "0077";
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        RestrictNamespaces = true;
        RestrictAddressFamilies = [
          "AF_UNIX"
          "AF_INET"
          "AF_INET6"
        ];
        SystemCallArchitectures = "native";
        MemoryMax = "512M";
        TasksMax = 32;
        LimitNOFILE = 128;
        Restart = "on-failure";
        RestartSec = 5;
      };
    };
  };
}
