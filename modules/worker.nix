{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.worker;
  backend = config.virtualisation.oci-containers.backend;
  runtime = lib.getExe pkgs.${backend};
  json = pkgs.formats.json { };
  enabledAgents = lib.filterAttrs (_: agent: agent.enable) cfg.agents;
  containerNames = lib.attrNames config.virtualisation.oci-containers.containers;
  backendUnits = lib.optional (backend == "docker") "docker.service";
  safePath = value: lib.match "^/var/lib/[A-Za-z0-9._+/-]+$" value != null;
  stateDir = name: "/var/lib/tentaflake-worker-${name}";
  resultDir = name: "${stateDir name}/results";
  workerGroup =
    agent:
    if agent.hostGroup != null then
      agent.hostGroup
    else if agent.containerGid == 65534 then
      "nogroup"
    else
      "tfw-gid-${toString agent.containerGid}";
  autoWorkerGroups = lib.foldl' (
    groups: agent:
    let
      group = workerGroup agent;
    in
    if agent.hostGroup == null && agent.containerGid != 65534 then
      groups // { ${group}.gid = agent.containerGid; }
    else
      groups
  ) { } (lib.attrValues enabledAgents);

  agentType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        enable = lib.mkEnableOption "disposable no-egress tool worker for ${name}";
        workspace = lib.mkOption {
          type = lib.types.str;
          default = "";
          example = "/var/lib/hermes-coding/workspace";
          description = "Exact persistent controller workspace copied into bounded worker snapshots.";
        };
        containerUid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = 10000;
        };
        containerGid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = 10000;
        };
        hostGroup = lib.mkOption {
          type = lib.types.nullOr (lib.types.strMatching "^[a-z_][a-z0-9_-]{0,30}$");
          default = null;
          description = ''
            Existing host group whose explicit GID matches containerGid. Null
            creates one deterministic group per non-nogroup container GID.
          '';
        };
        maxRequestBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 64 * 1024;
        };
        maxSnapshotBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1024 * 1024 * 1024;
        };
        maxSnapshotEntries = lib.mkOption {
          type = lib.types.ints.positive;
          default = 200000;
        };
        maxLogBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1024 * 1024;
        };
        maxTimeoutSeconds = lib.mkOption {
          type = lib.types.ints.positive;
          default = 900;
        };
        memory = lib.mkOption {
          type = lib.types.str;
          default = "2g";
        };
        memorySwap = lib.mkOption {
          type = lib.types.str;
          default = "2g";
        };
        cpus = lib.mkOption {
          type = lib.types.str;
          default = "2.0";
        };
        pidsLimit = lib.mkOption {
          type = lib.types.ints.positive;
          default = 512;
        };
        workspaceTmpfsSize = lib.mkOption {
          type = lib.types.str;
          default = "2g";
          description = "Hard tmpfs ceiling for the disposable writable workspace.";
        };
        tmpTmpfsSize = lib.mkOption {
          type = lib.types.str;
          default = "256m";
        };
      };
    }
  );

  workerConfig =
    name: agent:
    json.generate "tentaflake-worker-${name}.json" {
      agent = name;
      inherit backend runtime;
      image = cfg.imageReference;
      workspace = agent.workspace;
      state_dir = stateDir name;
      container_uid = agent.containerUid;
      container_gid = agent.containerGid;
      max_request_bytes = agent.maxRequestBytes;
      max_snapshot_bytes = agent.maxSnapshotBytes;
      max_snapshot_entries = agent.maxSnapshotEntries;
      max_log_bytes = agent.maxLogBytes;
      max_timeout_seconds = agent.maxTimeoutSeconds;
      inherit (agent) memory cpus;
      memory_swap = agent.memorySwap;
      pids_limit = agent.pidsLimit;
      workspace_tmpfs_size = agent.workspaceTmpfsSize;
      tmp_tmpfs_size = agent.tmpTmpfsSize;
    };

  workerService =
    name: agent:
    let
      quota = lib.attrByPath [ name ] null config.tentaflake.workspaceQuota.agents;
      quotaUnits = lib.optional (
        quota != null && quota.enable
      ) "tentaflake-workspace-quota-${name}.service";
    in
    {
      description = "Disposable tool-worker queue for ${name}";
      wantedBy = [ "multi-user.target" ];
      requires = [ "tentaflake-worker-image.service" ] ++ backendUnits ++ quotaUnits;
      after = [ "tentaflake-worker-image.service" ] ++ backendUnits ++ quotaUnits;
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        Group = workerGroup agent;
        UMask = "0027";
        ExecStart = "${lib.getExe cfg.package} --config ${workerConfig name agent} drain";
        Restart = "on-failure";
        RestartSec = "10s";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectSystem = "strict";
        ReadOnlyPaths = [ agent.workspace ];
        ReadWritePaths = [
          (stateDir name)
          "${agent.workspace}/.tentaflake-worker/inbox"
        ];
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
        SystemCallArchitectures = "native";
      };
      # This path-activated oneshot drains the whole queue on every successful
      # start. systemd's start limit counts those successes too, so a small
      # aggregate burst limit can permanently disable a healthy queue. Real
      # failures remain rate-bounded by RestartSec.
      startLimitIntervalSec = 0;
    };

  allServices = lib.mapAttrs' (
    name: agent: lib.nameValuePair "tentaflake-worker-${name}" (workerService name agent)
  ) enabledAgents;
  allPaths = lib.mapAttrs' (
    name: agent:
    let
      quota = lib.attrByPath [ name ] null config.tentaflake.workspaceQuota.agents;
      quotaUnits = lib.optional (
        quota != null && quota.enable
      ) "tentaflake-workspace-quota-${name}.service";
    in
    lib.nameValuePair "tentaflake-worker-${name}" {
      description = "Watch the ${name} tool-worker inbox";
      wantedBy = [ "multi-user.target" ];
      requires = quotaUnits;
      after = [
        "basic.target"
        "systemd-tmpfiles-setup.service"
      ]
      ++ quotaUnits;
      before = [ "shutdown.target" ];
      conflicts = [ "shutdown.target" ];
      # A path unit normally starts before paths.target (and therefore before
      # basic.target). Quota-backed inboxes are late mounts, so start this
      # watcher explicitly after the ownership unit instead.
      unitConfig.DefaultDependencies = false;
      pathConfig = {
        DirectoryNotEmpty = "${agent.workspace}/.tentaflake-worker/inbox";
        Unit = "tentaflake-worker-${name}.service";
      };
    }
  ) enabledAgents;
in
{
  options.tentaflake.worker = {
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-worker { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/tentaflake-worker { }";
      description = "Host-side disposable worker orchestrator.";
    };
    image = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-worker/image.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/tentaflake-worker/image.nix { }";
      description = "Nix-built OCI image loaded locally for disposable jobs.";
    };
    imageReference = lib.mkOption {
      type = lib.types.str;
      default = "tentaflake-worker:0.4.0";
      description = "Exact local reference emitted by worker.image; this is not pulled from a registry.";
    };
    agents = lib.mkOption {
      type = lib.types.attrsOf agentType;
      default = { };
      description = "Per-controller disposable worker and approval policy.";
    };
  };

  config = {
    assertions = [
      {
        assertion = enabledAgents == { } || config.tentaflake.security.profile != "dev";
        message = "tentaflake disposable workers require balanced or strict security profile.";
      }
      {
        assertion = enabledAgents == { } || config.tentaflake.hardening.enable;
        message = "tentaflake disposable workers require host hardening.";
      }
      {
        assertion =
          enabledAgents == { }
          || (cfg.imageReference != "" && lib.match "^[A-Za-z0-9._/:+-]+$" cfg.imageReference != null);
        message = "tentaflake worker imageReference must be one exact shell-safe local image reference.";
      }
    ]
    ++ lib.concatLists (
      lib.mapAttrsToList (
        name: agent:
        let
          group = workerGroup agent;
        in
        [
          {
            assertion = lib.elem name containerNames;
            message = "tentaflake worker key ${name} must exactly match an OCI agent container name.";
          }
          {
            assertion = lib.match "^[a-z0-9][a-z0-9-]{0,47}$" name != null;
            message = "tentaflake worker names must be lowercase safe OCI identifiers up to 48 characters.";
          }
          {
            assertion = safePath agent.workspace && !(lib.elem ".." (lib.splitString "/" agent.workspace));
            message = "tentaflake worker ${name} requires an explicit simple workspace below /var/lib.";
          }
          {
            assertion = agent.maxSnapshotBytes <= 16 * 1024 * 1024 * 1024;
            message = "tentaflake worker ${name} snapshot limit may not exceed 16 GiB.";
          }
          {
            assertion =
              lib.hasAttr group config.users.groups && config.users.groups.${group}.gid == agent.containerGid;
            message = "tentaflake worker ${name} host group ${group} must exist with GID ${toString agent.containerGid}.";
          }
        ]
      ) enabledAgents
    );

    environment.systemPackages = lib.optionals (enabledAgents != { }) [ cfg.package ];

    users.groups = autoWorkerGroups;

    environment.etc = lib.mapAttrs' (
      name: agent:
      lib.nameValuePair "tentaflake/workers/${name}.json" {
        source = workerConfig name agent;
        mode = "0444";
      }
    ) enabledAgents;

    services.logrotate.settings = lib.mapAttrs' (
      name: _:
      lib.nameValuePair "tentaflake-worker-${name}" {
        files = [ "${stateDir name}/audit.jsonl" ];
        frequency = "daily";
        rotate = 14;
        compress = true;
        delaycompress = true;
        missingok = true;
        notifempty = true;
        create = "0600 root root";
      }
    ) enabledAgents;

    systemd.tmpfiles.rules = lib.concatLists (
      lib.mapAttrsToList (
        name: agent:
        let
          group = workerGroup agent;
        in
        [
          "d ${stateDir name} 0750 root ${group} -"
          "d ${stateDir name}/pending 0700 root root -"
          "d ${stateDir name}/approvals 0700 root root -"
          "d ${stateDir name}/jobs 0700 root root -"
          "d ${resultDir name} 2750 root ${group} 14d"
          "d ${agent.workspace}/.tentaflake-worker 0700 ${toString agent.containerUid} ${group} -"
          "d ${agent.workspace}/.tentaflake-worker/inbox 0770 ${toString agent.containerUid} ${group} -"
        ]
      ) enabledAgents
    );

    systemd.services =
      allServices
      // lib.optionalAttrs (enabledAgents != { }) {
        tentaflake-worker-image = {
          description = "Load the Nix-built tentaflake disposable worker image";
          wantedBy = [ "multi-user.target" ];
          requires = backendUnits;
          after = backendUnits;
          before = lib.mapAttrsToList (name: _: "tentaflake-worker-${name}.service") enabledAgents;
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            ExecStart = "${runtime} load --input ${cfg.image}";
          };
        };
      };
    systemd.paths = allPaths;
  };
}
