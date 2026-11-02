{
  config,
  lib,
  pkgs,
  ...
}:
let
  defaultOwnership = (import ../lib/runtimeCatalog.nix).presets.generic.ownership;
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
    else if agent.containerGid == config.ids.gids.nogroup then
      "nogroup"
    else
      "tfw-gid-${toString agent.containerGid}";
  autoWorkerGroups = lib.foldl' (
    groups: agent:
    let
      group = workerGroup agent;
    in
    if agent.hostGroup == null && agent.containerGid != config.ids.gids.nogroup then
      groups // { ${group}.gid = agent.containerGid; }
    else
      groups
  ) { } (lib.attrValues enabledAgents);

  agentType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        enable = lib.mkEnableOption "disposable no-egress tool worker for ${name}";
        activity = lib.mkOption {
          default = null;
          description = "Optional host-owned fixed external-collector activity context, published through a dedicated root-authenticating non-root relay.";
          type = lib.types.nullOr (
            lib.types.submodule {
              options =
                (lib.genAttrs [ "producer_id" "tenant_id" "run_id" "workload_id" "policy_revision" ] (
                  _:
                  lib.mkOption {
                    type = lib.types.strMatching "^[A-Za-z0-9_.-]{1,64}$";
                  }
                ))
                // {
                  socket = lib.mkOption { type = lib.types.strMatching "^/run/[A-Za-z0-9_./-]+$"; };
                  server_uid = lib.mkOption { type = lib.types.ints.positive; };
                  producer_uid = lib.mkOption { type = lib.types.ints.positive; };
                  generation = lib.mkOption { type = lib.types.ints.positive; };
                };
            }
          );
        };
        activityUser = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "Dedicated existing static non-root relay user matching the collector's producer grant.";
        };
        workspace = lib.mkOption {
          type = lib.types.str;
          default = "";
          example = "/var/lib/tentaflake/agents/coding/workspace";
          description = "Exact persistent controller workspace copied into bounded worker snapshots.";
        };
        containerUid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = config.tentaflake.agentInstances.${name}.uid or defaultOwnership.uid;
        };
        containerGid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = config.tentaflake.agentInstances.${name}.gid or defaultOwnership.gid;
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
          description = "Largest accepted request file; Pi Durable's offline_worker tool requires at least 65536.";
        };
        maxInboxEntries = lib.mkOption {
          type = lib.types.ints.positive;
          default = 256;
          description = "Maximum top-level inbox entries, including invalid names and staging files.";
        };
        maxInboxBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 8 * 1024 * 1024;
          description = "Maximum summed inbox entry sizes; directory trees remain bounded by the workspace quota.";
        };
        maxQueueEntries = lib.mkOption {
          type = lib.types.ints.positive;
          default = 256;
          description = "Combined private pending and running request entry ceiling.";
        };
        maxQueueBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 8 * 1024 * 1024;
          description = "Combined private pending and running request byte ceiling.";
        };
        maxRetainedResults = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1024;
          description = "Combined retained result and in-flight job count ceiling; completed receipts are never deleted to admit work.";
        };
        maxRetainedResultBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 4 * 1024 * 1024 * 1024;
          description = "Aggregate retained payload byte ceiling. Admission reserves two full snapshots, a bounded log and 16 KiB metadata for every running job.";
        };
        maxRetainedEntries = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1000000;
          description = "Aggregate retained tree-entry ceiling, including worst-case running reservations.";
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
          default = 3600;
          description = "Largest accepted job timeout; Pi Durable's offline_worker tool requires at least 3600.";
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

  activityConfig =
    name: agent:
    agent.activity
    // {
      agent_id = name;
      relay_socket = "/run/tfw-activity-${name}/publish.sock";
    };

  workerConfig =
    name: agent:
    json.generate "tentaflake-worker-${name}.json" {
      agent = name;
      activity = if agent.activity == null then null else activityConfig name agent;
      inherit backend runtime;
      image = cfg.imageReference;
      inherit (agent) workspace;
      state_dir = stateDir name;
      container_uid = agent.containerUid;
      container_gid = agent.containerGid;
      max_request_bytes = agent.maxRequestBytes;
      max_inbox_entries = agent.maxInboxEntries;
      max_inbox_bytes = agent.maxInboxBytes;
      max_queue_entries = agent.maxQueueEntries;
      max_queue_bytes = agent.maxQueueBytes;
      max_retained_results = agent.maxRetainedResults;
      max_retained_result_bytes = agent.maxRetainedResultBytes;
      max_retained_entries = agent.maxRetainedEntries;
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
        # Rechecked when the service exits: unlike another start job, this
        # survives a decision recorded after drain unlocks but before exit.
        PathExists = "${stateDir name}/.drain-request";
        Unit = "tentaflake-worker-${name}.service";
      };
    }
  ) enabledAgents;
in
{
  options.tentaflake.worker = {
    publisherPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-activity { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/tentaflake-activity { }";
      description = "Isolated minimized activity publisher; no worker engine access.";
    };
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
      default = "${cfg.image.imageName}:${cfg.image.imageTag}";
      defaultText = lib.literalExpression ''"''${cfg.image.imageName}:''${cfg.image.imageTag}"'';
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
            assertion =
              agent.activity == null
              || (
                agent.activityUser != ""
                &&
                  (lib.attrByPath [ agent.activityUser "uid" ] null config.users.users) == agent.activity.producer_uid
                && !(lib.elem agent.activity.producer_uid (
                  map (instance: instance.uid) (lib.attrValues config.tentaflake.agentInstances)
                ))
                && (lib.attrByPath [ agent.activityUser "extraGroups" ] [ ] config.users.users) == [ ]
                && (lib.attrByPath [ agent.activityUser "group" ] "" config.users.users) == agent.activityUser
                && builtins.hasAttr agent.activityUser config.users.groups
                && builtins.stringLength agent.activity.socket < 108
                && lib.all (part: part != "" && part != "." && part != "..") (
                  lib.tail (lib.splitString "/" agent.activity.socket)
                )
                && agent.activity.generation <= 9007199254740991
                &&
                  lib.count
                    (
                      other:
                      other.enable && other.activity != null && other.activity.producer_uid == agent.activity.producer_uid
                    )
                    (
                      (lib.attrValues cfg.agents)
                      ++ (lib.attrValues (lib.attrByPath [ "tentaflake" "broker" "agents" ] { } config))
                    ) == 1
              );
            message = "Worker activity requires a dedicated static non-root publisher UID, no supplementary groups, and the collector's exact producer grant.";
          }
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
            assertion = agent.maxRetainedResultBytes >= 2 * agent.maxSnapshotBytes + agent.maxLogBytes + 16384;
            message = "tentaflake worker ${name} retained byte capacity must fit two snapshots, a log and 16 KiB metadata.";
          }
          {
            assertion = agent.maxRetainedEntries >= 2 * agent.maxSnapshotEntries + 8;
            message = "tentaflake worker ${name} retained entry capacity must fit one worst-case running job.";
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

    systemd = {
      tmpfiles.rules = lib.concatLists (
        lib.mapAttrsToList (
          name: agent:
          let
            group = workerGroup agent;
            quota = lib.attrByPath [ name ] null config.tentaflake.workspaceQuota.agents;
            workspaceQuotaEnabled = quota != null && quota.enable;
          in
          [
            "d ${stateDir name} 0750 root ${group} -"
            "d ${stateDir name}/pending 0700 root root -"
            "d ${stateDir name}/approvals 0700 root root -"
            "d ${stateDir name}/jobs 0700 root root -"
            "d ${stateDir name}/reservations 0700 root root -"
            "d ${resultDir name} 2750 root ${group} 14d"
          ]
          # Quota ownership initializes these only after the workspace mount.
          # Even sibling state/workspace layouts must not let tmpfiles create
          # private instance ancestry as 0755 or populate an unmounted source.
          ++ lib.optionals (!workspaceQuotaEnabled) [
            "d ${agent.workspace}/.tentaflake-worker 0700 ${toString agent.containerUid} ${group} -"
            "d ${agent.workspace}/.tentaflake-worker/inbox 0770 ${toString agent.containerUid} ${group} -"
          ]
        ) enabledAgents
      );

      services =
        allServices
        // lib.mapAttrs' (
          name: agent:
          lib.nameValuePair "tentaflake-worker-activity-${name}" {
            description = "Minimized fixed-context activity publisher for ${name}";
            wantedBy = [ "multi-user.target" ];
            before = [ "tentaflake-worker-${name}.service" ];
            serviceConfig = {
              User = agent.activityUser;
              Group = agent.activityUser;
              ExecStart = "${lib.getExe cfg.publisherPackage} ${json.generate "tfw-activity-${name}.json" (activityConfig name agent)}";
              RuntimeDirectory = "tfw-activity-${name}";
              RuntimeDirectoryMode = "0711";
              UMask = "0077";
              Restart = "on-failure";
              RestartSec = "5s";
              NoNewPrivileges = true;
              PrivateDevices = true;
              PrivateTmp = true;
              PrivateNetwork = true;
              ProtectSystem = "strict";
              ProtectHome = true;
              ProtectKernelTunables = true;
              ProtectKernelModules = true;
              ProtectControlGroups = true;
              RestrictAddressFamilies = [ "AF_UNIX" ];
              CapabilityBoundingSet = "";
              RestrictSUIDSGID = true;
              RestrictNamespaces = true;
              LockPersonality = true;
              InaccessiblePaths = [
                "-/run/docker.sock"
                "-/run/podman"
                "-/run/credentials"
                "-/var/lib/tentaflake-worker-${name}"
              ];
              MemoryMax = "32M";
              TasksMax = 1;
              LimitNOFILE = 32;
            };
          }
        ) (lib.filterAttrs (_: agent: agent.activity != null) enabledAgents)
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
      paths = allPaths;
    };
  };
}
