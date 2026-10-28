{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.hostApi;
  backend = config.virtualisation.oci-containers.backend;
  inventory = pkgs.writeText "tentaflake-api-agents.tsv" (
    (import ../lib/agentInventory.nix { inherit lib; }) {
      inherit backend;
      containers = config.virtualisation.oci-containers.containers;
      instances = config.tentaflake.agentInstances;
    }
  );
  cli = pkgs.callPackage ../pkgs/tentaflake-cli { };
  cliConfig = pkgs.writeText "tentaflake-api-cli.conf" ''
    backend=${backend}
    flake_dir=/etc/nixos
    host_name=${config.tentaflake.hostName}
    agents_file=${inventory}
    security_profile=${config.tentaflake.security.profile}
    security_file=/etc/tentaflake/security.tsv
  '';
  apiConfig = pkgs.writeText "tentaflake-host-api.json" (
    builtins.toJSON {
      host = config.tentaflake.hostName;
      inherit backend;
      security_profile = config.tentaflake.security.profile;
      allow_actions = cfg.allowActions;
      allow_logs = cfg.allowLogs;
      systemctl = "${pkgs.systemd}/bin/systemctl";
      journalctl = "${pkgs.systemd}/bin/journalctl";
      cli = lib.getExe cli;
      cli_config = toString cliConfig;
      command_path = lib.makeBinPath [
        pkgs.systemd
        pkgs.coreutils
      ];
      inventory = toString inventory;
      state = "/var/lib/tentaflake-api";
      stop_units =
        lib.mapAttrs
          (
            name: agent:
            lib.optional (agent.enable && agent.llm.enable) "tentaflake-broker-llm-${name}.service"
            ++ lib.optional (agent.enable && agent.fetch.enable) "tentaflake-broker-fetch-${name}.service"
          )
          (
            lib.filterAttrs (
              _: agent: agent.enable && (agent.llm.enable || agent.fetch.enable)
            ) config.tentaflake.broker.agents
          );
    }
  );
in
{
  options.tentaflake.hostApi = {
    enable = lib.mkEnableOption "the optional local operator API";
    allowActions = lib.mkEnableOption "fixed agent start, stop and restart operations";
    allowLogs = lib.mkEnableOption "bounded agent journal reads for trusted operators";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-host-api { };
      description = "Local operator API package.";
    };
  };
  config = lib.mkIf cfg.enable {
    users.groups.tentaflake-api = { };
    systemd.services.tentaflake-api = {
      description = "Optional local tentaflake operator API";
      wantedBy = [ "multi-user.target" ];
      after = [ "dbus.service" ];
      serviceConfig = {
        Type = "simple";
        User = "root";
        Group = "tentaflake-api";
        ExecStart = "${lib.getExe cfg.package} --config ${apiConfig}";
        RuntimeDirectory = "tentaflake-api";
        RuntimeDirectoryMode = "0750";
        StateDirectory = "tentaflake-api";
        StateDirectoryMode = "0700";
        UMask = "0077";
        Restart = "on-failure";
        RestartSec = "5s";
        TimeoutStopSec = "45s";
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictSUIDSGID = true;
        RestrictRealtime = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        TasksMax = 64;
        MemoryMax = "256M";
      };
    };
  };
}
