# Agent-independent OCI workload. Host policy owns the isolation backend.
{ pkgs, lib }:
let
  preset = (import ../lib/runtimeCatalog.nix).presets.generic;
  validate = import ../lib/runtimeContract.nix { inherit lib; };
  security = import ../lib/containerSecurity.nix { inherit lib; };
  limits = import ../lib/resourceLimits.nix { inherit lib; };
  metadata =
    args: backend:
    let
      definition = validate args.definition;
      container = "generic-${args.name}";
      stateDir = "/var/lib/${container}";
    in
    {
      adapter = "generic";
      inherit (args) name;
      inherit container stateDir;
      stateStorage = "${stateDir}/state";
      unit = "${backend}-${container}.service";
      workspace = "${stateDir}/workspace";
      inherit (definition.ownership) uid gid;
      runnable = true;
      requestedCapabilities = definition.capabilities;
      requireStateQuota = true;
    };
in
{
  inherit (preset)
    schemaVersion
    identity
    artifact
    command
    ownership
    layout
    lifecycle
    model
    execution
    capabilities
    evidence
    ;
  configuration = preset.configuration // {
    generate = _: _: throw "tentaflake: generic workloads have no native configuration hook.";
  };
  research = preset.research // {
    configure = _: {
      settings = { };
      valid = false;
    };
  };
  inherit metadata;
  build =
    { name, definition }:
    let
      d = validate definition;
    in
    builtins.deepSeq d (
      { config, ... }:
      let
        backend = config.virtualisation.oci-containers.backend;
        instance = metadata { inherit name definition; } backend;
        inherit (instance)
          container
          stateDir
          stateStorage
          workspace
          uid
          gid
          ;
        owner = "${toString uid}:${toString gid}";
        requested = capability: lib.elem capability d.capabilities;
        ceilings = config.tentaflake.security.resources;
        resources = ceilings // d.resources;
        resourceValue =
          key: value:
          if key == "cpus" then
            limits.cpuNano value
          else if
            lib.elem key [
              "nofile"
              "pidsLimit"
            ]
          then
            value
          else
            limits.sizeBytes (
              if
                lib.elem key [
                  "tmpfsSize"
                  "runTmpfsSize"
                ]
              then
                limits.maxTmpfsBytes
              else
                limits.maxMemoryBytes
            ) value;
        broker = lib.attrByPath [ container ] null config.tentaflake.broker.agents;
        brokerEnabled = broker != null && broker.enable;
        modelEnabled = brokerEnabled && broker.llm.enable;
        brokerEnvironmentFile = "/run/tentaflake-broker/${container}/agent.env";
        research = import ../lib/researchClient.nix {
          inherit config lib pkgs;
          containerName = container;
          settings = null;
        };
        worker = lib.attrByPath [ container ] null config.tentaflake.worker.agents;
        workerEnabled = worker != null && worker.enable;
        workerResults = "/var/lib/tentaflake-worker-${container}/results";
        quota = lib.attrByPath [ container ] null config.tentaflake.workspaceQuota.agents;
        quotaEnabled = quota != null && quota.enable;
        stateQuotaEnabled = quotaEnabled && quota.state != null;
        dependencies =
          lib.optional quotaEnabled "tentaflake-workspace-quota-${container}.service"
          ++ lib.optionals modelEnabled [
            "tentaflake-broker-network-${container}.service"
            "tentaflake-broker-llm-${container}.service"
          ]
          ++ lib.optionals workerEnabled [
            "tentaflake-worker-image.service"
            "tentaflake-worker-${container}.path"
          ]
          ++ research.units;
        result = security.apply {
          profile = config.tentaflake.security.profile;
          inherit backend owner;
          name = container;
          inherit resources stateQuotaEnabled;
          inherit (resources) pidsLimit;
          requestedCapabilities = d.capabilities;
          requireStateQuota = true;
          brokerNetwork = if modelEnabled then broker.networkName else null;
          approvedEnvironmentFiles = lib.optional modelEnabled brokerEnvironmentFile;
          allowedWritableSources = [
            stateStorage
            workspace
          ];
          allowedWritableDestinations = [
            "/state"
            d.workspace
          ];
          approvedReadOnlySources = lib.optional workerEnabled workerResults ++ research.readOnlySources;
          approvedReadOnlyDestinations =
            lib.optional workerEnabled "/run/tentaflake-worker/results" ++ research.readOnlyDestinations;
          baseConfig = {
            inherit (d) image;
            cmd = d.command;
            autoStart = d.lifecycle == "service";
            user = owner;
            environment = {
              HOME = "/state";
            }
            // lib.optionalAttrs research.enabled {
              TENTAFLAKE_RESEARCH_COMMAND = research.server.command;
              TENTAFLAKE_RESEARCH_SOCKET = "/run/tentaflake-research/socket";
            };
            environmentFiles = lib.optional modelEnabled brokerEnvironmentFile;
            networks = lib.optional modelEnabled broker.networkName;
            volumes = [
              "${stateStorage}:/state:rw"
              "${workspace}:${d.workspace}:rw"
            ]
            ++ lib.optional workerEnabled "${workerResults}:/run/tentaflake-worker/results:ro"
            ++ research.volumes;
            extraOptions = [ "--workdir=${d.workspace}" ];
          };
          brokerPolicyEnabled = modelEnabled;
          researchPolicyEnabled = research.enabled;
          inherit workerEnabled;
          workspaceQuotaEnabled = quotaEnabled;
          automaticStart = config.virtualisation.oci-containers.containers.${container}.autoStart;
        };
      in
      {
        assertions = result.assertions ++ [
          {
            assertion = !brokerEnabled || (requested "model" && modelEnabled && !broker.fetch.enable);
            message = "tentaflake: generic model broker must match its requested capability and disable the legacy fetch broker.";
          }
          {
            assertion = !research.enabled || requested "research";
            message = "tentaflake: generic Research relay must match its requested capability.";
          }
          {
            assertion = config.tentaflake.security.profile != "dev";
            message = "tentaflake: generic workloads require the common secure runtime; dev is legacy compatibility only.";
          }
          {
            assertion =
              !workerEnabled
              || (
                requested "worker"
                && worker.workspace == workspace
                && worker.containerUid == uid
                && worker.containerGid == gid
              );
            message = "tentaflake: generic worker declarations must match requested capability, workspace and ownership.";
          }
          {
            assertion =
              !quotaEnabled || (quota.workspace == workspace && quota.ownerUid == uid && quota.ownerGid == gid);
            message = "tentaflake: generic workspace quota must match the isolated workspace and ownership.";
          }
          {
            assertion = !stateQuotaEnabled || quota.state.path == stateStorage;
            message = "tentaflake: generic private state quota must match the isolated state and ownership.";
          }
          {
            assertion = lib.all (key: resourceValue key resources.${key} <= resourceValue key ceilings.${key}) (
              builtins.attrNames d.resources
            );
            message = "tentaflake: generic resource requests must not exceed the host security resource ceilings.";
          }
          {
            assertion =
              resourceValue "memorySwap" resources.memorySwap >= resourceValue "memory" resources.memory;
            message = "tentaflake: generic effective memorySwap must be greater than or equal to memory.";
          }
        ];
        systemd.tmpfiles.rules = [
          "d ${stateDir} 0700 root root -"
          "d ${stateStorage} 0700 ${toString uid} ${toString gid} -"
          "d ${workspace} 0700 ${toString uid} ${toString gid} -"
        ];
        systemd.services."${backend}-${container}" = (import ../lib/serviceRecovery.nix) // {
          requires = [ "systemd-tmpfiles-setup.service" ] ++ dependencies;
          after = [ "systemd-tmpfiles-setup.service" ] ++ dependencies;
          preStart = lib.mkBefore ''
            ${lib.optionalString (result.missingAdmission != [ ]) ''
              echo ${lib.escapeShellArg "Generic runtime admission refused: missing ${lib.concatStringsSep ", " result.missingAdmission}."} >&2
              exit 1
            ''}
            for directory in ${
              lib.escapeShellArgs [
                stateDir
                stateStorage
                workspace
              ]
            }; do
              if [ ! -d "$directory" ] || [ -L "$directory" ]; then
                echo "Generic runtime requires real state and workspace directories, not symlinks." >&2
                exit 1
              fi
            done
            if [ "$(${pkgs.coreutils}/bin/stat -c %u:%a ${lib.escapeShellArg stateDir})" != 0:700 ]; then
              echo "Generic runtime requires a root-owned, mode-0700 instance parent." >&2
              exit 1
            fi
          '';
        };
        virtualisation.oci-containers.containers.${container} = result.container;
      }
    );
}
