{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.security;
  secure = cfg.profile != "dev";
  bool = value: if value then "true" else "false";
  inherit (import ../lib/resourceLimits.nix { inherit lib; })
    maxMemoryBytes
    maxTmpfsBytes
    validSize
    sizeBytes
    validCpus
    validResource
    ociResources
    ociTmpfs
    ;
  allOptions = container: (container.extraOptions or [ ]) ++ (container.preRunExtraOptions or [ ]);
  hasOption =
    prefix: container:
    lib.any (option: option == prefix || lib.hasPrefix "${prefix}=" option) (allOptions container);
  pathWithin = root: path: path == root || lib.hasPrefix "${root}/" path;
  dangerousMountRoots = [
    "/"
    "/dev"
    "/etc"
    "/home"
    "/proc"
    "/root"
    "/run"
    "/sys"
    "/var/lib/containers"
    "/var/lib/docker"
    "/var/lib/tentaflake-worker-state-volumes"
    "/var/lib/tentaflake-workspace-volumes"
    "/var/run"
  ];
  mountSafe =
    name: volume:
    let
      parts = lib.splitString ":" volume;
      source = if parts == [ ] then "" else lib.head parts;
    in
    (
      builtins.hasAttr name (lib.attrByPath [ "tentaflake" "research" "agents" ] { } config)
      && volume == "/run/tentaflake-research/${name}:/run/tentaflake-research:ro"
    )
    || (
      source != ""
      && !(lib.any (root: pathWithin root source) dangerousMountRoots)
      && !(lib.hasSuffix ".sock" source)
    );
  portPrivate = port: lib.hasPrefix "127.0.0.1:" port || lib.hasPrefix "[::1]:" port;
  nonRootUser = user: user != null && lib.match "^[1-9][0-9]*:[1-9][0-9]*$" user != null;
  resourcesComplete =
    container:
    lib.all (prefix: hasOption prefix container) [
      "--cpus"
      "--memory"
      "--memory-swap"
      "--pids-limit"
      "--ulimit"
    ];
  agentContainers = lib.filterAttrs (
    _: container: (container.labels."io.tentaflake.agent" or "false") == "true"
  ) config.virtualisation.oci-containers.containers;
  agentRecord =
    name: container:
    let
      user = container.user or null;
      capabilitiesEmpty =
        !(lib.any (value: value == true) (lib.attrValues (container.capabilities or { })));
      imagePinned = lib.match ".+@sha256:[0-9a-fA-F]{64}" container.image != null;
      brokered = (container.labels."io.tentaflake.brokered-egress" or "false") == "true";
      brokerPolicy = lib.attrByPath [ name ] null config.tentaflake.broker.agents;
      llmBrokerEnabled = brokerPolicy != null && brokerPolicy.enable && brokerPolicy.llm.enable;
      fetchBrokerEnabled = brokerPolicy != null && brokerPolicy.enable && brokerPolicy.fetch.enable;
      llmEndpoint =
        if llmBrokerEnabled then "${brokerPolicy.gateway}:${toString brokerPolicy.llm.port}" else "-";
      fetchEndpoint =
        if fetchBrokerEnabled then "${brokerPolicy.gateway}:${toString brokerPolicy.fetch.port}" else "-";
      brokerNetwork = if brokered && brokerPolicy != null then brokerPolicy.networkName else "-";
      worker = lib.attrByPath [ name ] null config.tentaflake.worker.agents;
      workerEnabled = worker != null && worker.enable;
      quota = lib.attrByPath [ name ] null config.tentaflake.workspaceQuota.agents;
      quotaEnabled = quota != null && quota.enable;
      seccompConfined = !hasOption "--security-opt=seccomp=unconfined" container;
      apparmorConfined =
        if config.virtualisation.oci-containers.backend == "docker" then
          hasOption "--security-opt=apparmor=docker-default" container
        else
          config.security.apparmor.enable && !hasOption "--security-opt=apparmor=unconfined" container;
      provenancePolicy = lib.attrByPath [ name ] null config.tentaflake.imageProvenance.agents;
      provenanceGateConfigured = provenancePolicy != null && provenancePolicy.enable;
      liveResources = ociResources (allOptions container);
      liveTmpfs = ociTmpfs (allOptions container);
      declaredMounts = if secure then builtins.toJSON (container.volumes or [ ]) else "-";
      declaredTmpfs = if secure && liveTmpfs != null then builtins.toJSON liveTmpfs else "-";
      declaredPortsAbsent = if secure then bool ((container.ports or [ ]) == [ ]) else "-";
      declaredLiveResources =
        if secure && liveResources != null then builtins.toJSON liveResources else "-";
      instance = config.tentaflake.agentInstances.${name} or null;
      declaredCapabilities =
        if instance == null then
          "-"
        else
          builtins.toJSON {
            schemaVersion = 1;
            requested = instance.requestedCapabilities;
            research = builtins.hasAttr name (lib.attrByPath [ "tentaflake" "research" "agents" ] { } config);
            stateQuota = quotaEnabled && quota.state != null;
            inherit (instance) requireStateQuota;
          };
      fields = [
        "agent"
        name
        (container.labels."io.tentaflake.security-profile" or "unknown")
        (bool (hasOption "--network=none" container || brokered))
        (bool (lib.all portPrivate (container.ports or [ ])))
        (bool (!(container.privileged or false)))
        (bool (nonRootUser user))
        (bool capabilitiesEmpty)
        (bool (hasOption "--security-opt=no-new-privileges:true" container))
        (bool (hasOption "--read-only" container))
        (bool (lib.all (mountSafe name) (container.volumes or [ ])))
        (bool (
          hasOption "--runtime=runsc" container || lib.elem "runsc" (container.preRunExtraOptions or [ ])
        ))
        (bool (resourcesComplete container))
        (bool imagePinned)
        (bool ((container.environmentFiles or [ ]) == [ ] || brokered))
        (bool workerEnabled)
        (bool quotaEnabled)
        (bool seccompConfined)
        (bool apparmorConfined)
        (bool provenanceGateConfigured)
        (bool brokered)
        (bool llmBrokerEnabled)
        llmEndpoint
        (bool fetchBrokerEnabled)
        fetchEndpoint
        brokerNetwork
        declaredMounts
        declaredTmpfs
        declaredPortsAbsent
        declaredLiveResources
        declaredCapabilities
      ];
    in
    lib.concatStringsSep "\t" fields;
  securityManifest = lib.concatStringsSep "\n" (
    [
      "manifest\t3"
      (lib.concatStringsSep "\t" [
        "host"
        cfg.profile
        (bool config.services.openssh.enable)
        (bool (
          agentContainers != { }
          && lib.all (container: (container.labels."io.tentaflake.brokered-egress" or "false") == "true") (
            lib.attrValues agentContainers
          )
        ))
        (bool config.security.apparmor.enable)
        (bool (!lib.elem "docker" (config.users.users.${config.tentaflake.adminUser}.extraGroups or [ ])))
        (bool config.tentaflake.backup.enable)
        (toString config.tentaflake.backup.lastSuccessMaxAgeHours)
      ])
      (lib.concatStringsSep "\t" [
        "management"
        config.tentaflake.management.transport
        (bool config.tentaflake.management.enable)
        (bool config.tentaflake.management.capabilities.privateConnectivity)
        config.tentaflake.management.capabilities.sshAuthorization
      ])
    ]
    ++ lib.mapAttrsToList agentRecord agentContainers
  );
in
{
  options.tentaflake.security = {
    profile = lib.mkOption {
      type = lib.types.enum [
        "dev"
        "balanced"
        "strict"
      ];
      default = "balanced";
      description = ''
        Agent security profile. balanced is the installed-host default. dev is
        a compatibility profile and is not suitable for untrusted 24/7 agents.
        strict currently fails evaluation rather than falling back to a weaker
        kernel boundary.
      '';
    };

    resources = {
      memory = lib.mkOption {
        type = lib.types.str;
        default = "2g";
        description = "Per-agent memory limit passed to the OCI runtime.";
      };
      memorySwap = lib.mkOption {
        type = lib.types.str;
        default = "2g";
        description = "Per-agent memory plus swap ceiling. Equal to memory disables additional swap.";
      };
      cpus = lib.mkOption {
        type = lib.types.str;
        default = "2.0";
        description = "Per-agent CPU quota in OCI --cpus syntax.";
      };
      nofile = lib.mkOption {
        type = lib.types.ints.positive;
        default = 4096;
        description = "Per-agent open-file ulimit.";
      };
      pidsLimit = lib.mkOption {
        type = lib.types.ints.positive;
        default = 512;
        description = "Maximum process count for a generic workload, including threads.";
      };
      tmpfsSize = lib.mkOption {
        type = lib.types.str;
        default = "256m";
        description = "Size of each general-purpose secure tmpfs mount.";
      };
      runTmpfsSize = lib.mkOption {
        type = lib.types.str;
        default = "64m";
        description = "Size of the secure /run tmpfs mount.";
      };
    };
  };

  config = lib.mkMerge [
    {
      assertions = [
        {
          assertion = cfg.profile != "strict";
          message = ''
            tentaflake.security.profile = "strict" is intentionally unavailable:
            the repository does not yet implement or test a separate-kernel
            MicroVM boundary. Use balanced, or keep strict as a deployment
            requirement until that boundary exists. No fallback was applied.
          '';
        }
      ]
      ++ lib.optionals secure [
        {
          assertion = config.tentaflake.hardening.enable;
          message = "tentaflake: balanced/strict requires tentaflake.hardening.enable = true.";
        }
        {
          assertion = config.security.apparmor.enable;
          message = "tentaflake: balanced/strict requires AppArmor on the host.";
        }
        {
          assertion = config.tentaflake.management.capabilities.privateConnectivity;
          message = "tentaflake: balanced/strict requires a configured private management transport.";
        }
        {
          assertion = config.tentaflake.management.capabilities.sshAuthorization != "disabled";
          message = "tentaflake: balanced/strict requires a configured private SSH authorization policy.";
        }
        {
          assertion = !config.services.openssh.enable;
          message = "tentaflake: balanced/strict forbids the public OpenSSH module; use the reviewed private management SSH policy.";
        }
        {
          assertion =
            validSize maxMemoryBytes cfg.resources.memory && validSize maxMemoryBytes cfg.resources.memorySwap;
          message = "tentaflake: secure memory and memorySwap limits must be positive byte sizes no greater than 1 TiB, with an optional K/M/G/T suffix.";
        }
        {
          assertion =
            !(validSize maxMemoryBytes cfg.resources.memory)
            || !(validSize maxMemoryBytes cfg.resources.memorySwap)
            ||
              sizeBytes maxMemoryBytes cfg.resources.memorySwap >= sizeBytes maxMemoryBytes cfg.resources.memory;
          message = "tentaflake: secure memorySwap must be greater than or equal to memory.";
        }
        {
          assertion = validCpus cfg.resources.cpus;
          message = "tentaflake: secure cpus must be a positive decimal no greater than 1024 with at most five fractional digits.";
        }
        {
          assertion =
            validResource "nofile" cfg.resources.nofile && validResource "pidsLimit" cfg.resources.pidsLimit;
          message = "tentaflake: secure file and process limits must be positive integers no greater than 4294967294.";
        }
        {
          assertion =
            validSize maxTmpfsBytes cfg.resources.tmpfsSize
            && validSize maxTmpfsBytes cfg.resources.runTmpfsSize;
          message = "tentaflake: secure tmpfs sizes must be positive byte sizes no greater than 64 GiB, with an optional K/M/G/T suffix.";
        }
      ];

      warnings = lib.optional (cfg.profile == "dev") ''
        tentaflake security profile "dev" preserves compatibility and is not a
        security boundary for untrusted 24/7 agents.
      '';

      environment.etc."tentaflake/security.tsv".text = securityManifest + "\n";
    }

    (lib.mkIf secure {
      environment.systemPackages = [ pkgs.gvisor ];

      virtualisation.docker.daemon.settings.runtimes.runsc = {
        path = lib.getExe' pkgs.gvisor "runsc";
      };

      virtualisation.podman.extraRuntimes = [ pkgs.gvisor ];
    })
  ];
}
