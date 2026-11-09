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
  workers = lib.filterAttrs (_: agent: agent.enable) config.tentaflake.worker.agents;
  policies = import ../lib/hostApiPolicy.nix { inherit config lib; };
  pi-durable = config.tentaflake.piDurableRuntime;
  piDurableTransport = import ../lib/piDurableTransport.nix { inherit lib pkgs; };
  runnerAdmitted =
    pi-durable.enable
    && pi-durable.package != null
    && pi-durable.bundle != null
    && pi-durable.image != null
    && (pi-durable.bundle.imageVerificationVersion or null) == 1
    && (pi-durable.bundle.imageSelector or null) == pi-durable.image
    && (pi-durable.bundle.runtimePackage or null) == pi-durable.package
    && piDurableTransport.supported config;
  runners =
    lib.mapAttrs
      (_: declaration: {
        socket = "/run/tentaflake-pi-durable/${declaration.container}/control/runner.sock";
        token = "/run/tentaflake-pi-durable/${declaration.container}/api-token";
        transport = piDurableTransport.version;
      })
      (
        lib.filterAttrs (
          _: declaration:
          declaration.adapter == "pi-durable"
          && declaration.runnable
          && runnerAdmitted
          && piDurableTransport.containerSupported (
            config.virtualisation.oci-containers.containers.${declaration.container} or { }
          )
        ) config.tentaflake.agentInstances
      );
  cliConfig = pkgs.writeText "tentaflake-api-cli.conf" ''
    backend=${backend}
    flake_dir=/etc/nixos
    host_name=${config.tentaflake.hostName}
    configuration_name=${config.tentaflake.configurationName}
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
      allow_tool_requests = cfg.allowToolRequests;
      allow_tool_decisions = cfg.allowToolDecisions;
      peer_grants = cfg.peers;
      inherit policies;
      inherit runners;
      worker =
        if cfg.allowToolRequests && workers != { } then
          lib.getExe config.tentaflake.worker.package
        else
          null;
      worker_configs = lib.optionalAttrs cfg.allowToolRequests (
        lib.mapAttrs (
          name: _: toString config.environment.etc."tentaflake/workers/${name}.json".source
        ) workers
      );
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
    allowToolRequests = lib.mkEnableOption "bounded reads of captured private worker requests";
    allowToolDecisions = lib.mkEnableOption "digest-bound approval and denial of captured worker requests";
    peers = lib.mkOption {
      default = null;
      description = ''
        Exact Unix peer UID and per-agent operation grants. Null retains the
        legacy trusted operator-group mode; enterprise clients require explicit
        grants. An empty list denies all callers, including root.
      '';
      type = lib.types.nullOr (
        lib.types.listOf (
          lib.types.submodule {
            options = {
              uid = lib.mkOption {
                type = lib.types.ints.between 1 4294967294;
              };
              agents = lib.mkOption {
                type = lib.types.listOf lib.types.str;
                default = [ ];
              };
              permissions = lib.mkOption {
                type = lib.types.listOf (
                  lib.types.enum [
                    "read"
                    "actions"
                    "logs"
                    "propose"
                    "tool_requests"
                    "tool_decisions"
                    "host_health"
                    "run_read"
                    "run_submit"
                    "run_stop"
                  ]
                );
                default = [ ];
              };
            };
          }
        )
      );
    };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-host-api { };
      description = "Local operator API package.";
    };
  };
  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = !cfg.allowToolDecisions || cfg.allowToolRequests;
        message = "tentaflake.hostApi.allowToolDecisions requires allowToolRequests so operators can inspect the captured request.";
      }
      {
        assertion =
          cfg.peers == null
          || (
            builtins.stringLength (builtins.toJSON cfg.peers) <= 512 * 1024
            &&
              builtins.length (map (peer: peer.uid) cfg.peers)
              == builtins.length (lib.unique (map (peer: peer.uid) cfg.peers))
            && lib.all (
              peer:
              builtins.length peer.agents == builtins.length (lib.unique peer.agents)
              && lib.all (id: builtins.hasAttr id config.tentaflake.agentInstances) peer.agents
              && builtins.length peer.permissions == builtins.length (lib.unique peer.permissions)
            ) cfg.peers
          );
        message = "tentaflake.hostApi.peers requires unique UIDs, unique permissions and exact declared agent IDs within 512 KiB of JSON.";
      }
    ];
    users.groups.tentaflake-api = { };
    systemd.services.tentaflake-api = {
      description = "Optional local tentaflake operator API";
      wantedBy = [ "multi-user.target" ];
      after = [ "dbus.service" ];
      serviceConfig = {
        SupplementaryGroups = lib.optional (runners != { }) "10100";
        Type = "simple";
        User = "root";
        Group = "tentaflake-api";
        ExecStart = "${lib.getExe cfg.package} --config ${apiConfig}";
        RuntimeDirectory = "tentaflake-api";
        RuntimeDirectoryMode = if cfg.peers == null then "0750" else "0711";
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
        ReadWritePaths = lib.optionals cfg.allowToolDecisions (
          lib.mapAttrsToList (name: _: "-/var/lib/tentaflake-worker-${name}") workers
        );
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
