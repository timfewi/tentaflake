{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.security;
  secure = cfg.profile != "dev";
  piDurableTransport = import ../lib/piDurableTransport.nix { inherit lib pkgs; };
  researchTransport = import ../lib/researchTransport.nix { inherit lib pkgs; };
  adapters = import ../adapters { inherit lib pkgs; };
  runtimes = import ../lib/gvisorRuntimes.nix { inherit lib pkgs; };
  piDurableTransportEnabled =
    lib.attrByPath [ "tentaflake" "piDurableRuntime" "enable" ] false config
    &&
      lib.attrByPath [ "tentaflake" "piDurableRuntime" "transport" ] null config
      == piDurableTransport.version;
  bool = value: if value then "true" else "false";
  maxMemoryBytes = 1024 * 1024 * 1024 * 1024;
  maxTmpfsBytes = 64 * 1024 * 1024 * 1024;
  maxCpus = 1024;
  sizeComponents =
    value:
    let
      matched = builtins.match "^([1-9][0-9]*)([kKmMgGtT]?)$" value;
      factors = {
        "" = 1;
        k = 1024;
        m = 1024 * 1024;
        g = 1024 * 1024 * 1024;
        t = 1024 * 1024 * 1024 * 1024;
      };
    in
    if matched == null || builtins.stringLength (lib.head matched) > 13 then
      null
    else
      {
        amount = builtins.fromJSON (lib.head matched);
        factor = factors.${lib.toLower (lib.elemAt matched 1)};
      };
  validSize =
    maximum: value:
    let
      components = sizeComponents value;
    in
    components != null && components.amount <= maximum / components.factor;
  sizeBytes =
    maximum: value:
    let
      components = sizeComponents value;
    in
    if components == null || components.amount > maximum / components.factor then
      0
    else
      components.amount * components.factor;
  digitValues = {
    "0" = 0;
    "1" = 1;
    "2" = 2;
    "3" = 3;
    "4" = 4;
    "5" = 5;
    "6" = 6;
    "7" = 7;
    "8" = 8;
    "9" = 9;
  };
  digitsToInt =
    value:
    lib.foldl' (total: digit: total * 10 + digitValues.${digit}) 0 (lib.stringToCharacters value);
  cpuComponents =
    value:
    let
      matched = builtins.match "^([0-9]+)([.]([0-9]+))?$" value;
      fraction = if matched == null || lib.elemAt matched 2 == null then "" else lib.elemAt matched 2;
    in
    if
      matched == null
      || builtins.stringLength (lib.head matched) > 4
      || builtins.stringLength fraction > 5
    then
      null
    else
      {
        whole = digitsToInt (lib.head matched);
        inherit fraction;
      };
  cpuNano =
    value:
    let
      components = cpuComponents value;
      fractionLength = if components == null then 0 else builtins.stringLength components.fraction;
      fractionFactor = lib.foldl' (total: _: total * 10) 1 (lib.replicate (9 - fractionLength) null);
    in
    if components == null then
      0
    else
      components.whole * 1000000000 + digitsToInt components.fraction * fractionFactor;
  validCpus =
    value:
    let
      nano = cpuNano value;
    in
    builtins.stringLength value <= 16
    && builtins.match "^([1-9][0-9]*([.][0-9]+)?|0[.][0-9]*[1-9][0-9]*)$" value != null
    && nano > 0
    && nano <= maxCpus * 1000000000;
  allOptions = container: (container.extraOptions or [ ]) ++ (container.preRunExtraOptions or [ ]);
  hasOption =
    prefix: container:
    lib.any (option: option == prefix || lib.hasPrefix "${prefix}=" option) (allOptions container);
  optionValue =
    prefix: container:
    let
      values = lib.filter (lib.hasPrefix prefix) (allOptions container);
    in
    if builtins.length values == 1 then lib.removePrefix prefix (lib.head values) else null;
  positiveOptionInt =
    prefix: container:
    let
      value = optionValue prefix container;
      parsed =
        if value != null && builtins.match "^[1-9][0-9]*$" value != null then
          builtins.tryEval (builtins.fromJSON value)
        else
          {
            success = false;
            value = 0;
          };
    in
    if parsed.success && parsed.value > 0 then parsed.value else 0;
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
    name: piDurableRuntimeValid: volume:
    let
      parts = lib.splitString ":" volume;
      source = if parts == [ ] then "" else lib.head parts;
    in
    (
      builtins.hasAttr name (lib.attrByPath [ "tentaflake" "research" "agents" ] { } config)
      && volume == "/run/tentaflake-research/${name}:/run/tentaflake-research:ro"
    )
    || (
      # Only the fully admitted Pi Durable runtime receives these exact per-agent binds.
      piDurableRuntimeValid
      && lib.elem volume [
        "/run/tentaflake-pi-durable/${name}/control:/run/tentaflake-pi-durable/control:rw"
        "/run/tentaflake-pi-durable/${name}/api-token:/run/tentaflake-pi-durable/api-token:ro"
        "/run/tentaflake-pi-durable/${name}/broker-token:/run/tentaflake-pi-durable/broker-token:ro"
      ]
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
  piDurableImageReviewed =
    name: container:
    let
      policy = lib.attrByPath [ "tentaflake" "piDurableRuntime" ] { } config;
      bundle = policy.bundle or null;
    in
    lib.isDerivation bundle
    && (bundle.imageVerificationVersion or null) == 1
    && (bundle.runtimePackage or null) == (policy.package or null)
    && (bundle.imageSelector or null) == container.image
    && (policy.image or null) == container.image
    && (container.imageFile or null) == "${bundle}/image.tar.gz"
    && (container.pull or null) == "never"
    &&
      lib.elem
        (toString (
          import ../lib/verifyLocalImage.nix {
            inherit lib pkgs bundle;
            backend = config.virtualisation.oci-containers.backend;
            inherit (container) image;
          }
        ))
        (
          map toString (
            lib.attrByPath
              [
                "systemd"
                "services"
                "${config.virtualisation.oci-containers.backend}-${name}"
                "serviceConfig"
                "ExecStartPre"
              ]
              [ ]
              config
          )
        );
  runtimeValidity =
    name: container:
    let
      selected = researchTransport.selected config.virtualisation.oci-containers.backend container;
      registered = runtimes.configurationSupported config;
      instance = lib.attrByPath [ name ] null config.tentaflake.agentInstances;
      researchEnabled = builtins.hasAttr name (
        lib.attrByPath [ "tentaflake" "research" "agents" ] { } config
      );
      piDurableRuntimeValid =
        registered
        && selected piDurableTransport.name
        && piDurableTransport.supported config
        && instance != null
        && instance.adapter == "pi-durable"
        && instance.runnable
        && piDurableTransport.containerSupported container
        && piDurableImageReviewed name container;
      researchRuntimeValid =
        registered
        && researchEnabled
        && instance != null
        && instance.runnable
        && instance.adapter != "pi-durable"
        && (adapters.${instance.adapter}.research.supported or false)
        && researchTransport.supported config
        && selected researchTransport.name;
      ordinaryRuntimeValid =
        registered
        && !researchEnabled
        && (instance == null || instance.adapter != "pi-durable")
        && selected runtimes.stock.name;
      managed = (container.labels."io.tentaflake.agent" or "false") == "true";
    in
    {
      inherit piDurableRuntimeValid researchRuntimeValid ordinaryRuntimeValid;
      allowed =
        if managed then
          piDurableRuntimeValid || researchRuntimeValid || ordinaryRuntimeValid
        else
          !(lib.any (runtimes.selectorElevated config) (runtimes.selectors container));
    };
  agentRecord =
    name: container:
    let
      user = container.user or null;
      capabilitiesEmpty =
        !(lib.any (value: value == true) (lib.attrValues (container.capabilities or { })));
      runtime = runtimeValidity name container;
      piDurableImagePinned = runtime.piDurableRuntimeValid;
      imagePinned = lib.match ".+@sha256:[0-9a-fA-F]{64}" container.image != null || piDurableImagePinned;
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
      inherit (runtime) piDurableRuntimeValid researchRuntimeValid ordinaryRuntimeValid;
      declaredRuntime =
        if !secure then
          "-"
        else if piDurableRuntimeValid then
          builtins.toJSON {
            inherit (piDurableTransport) name path;
          }
        else if researchRuntimeValid then
          builtins.toJSON {
            inherit (researchTransport) name path;
          }
        else if ordinaryRuntimeValid then
          builtins.toJSON {
            name = "runsc";
            path = lib.getExe' pkgs.gvisor "runsc";
          }
        else
          "-";
      declaredPidsLimit = positiveOptionInt "--pids-limit=" container;
      declaredMounts = if secure then builtins.toJSON (container.volumes or [ ]) else "-";
      declaredTmpfs =
        if secure then
          builtins.toJSON {
            "/run" = sizeBytes maxTmpfsBytes cfg.resources.runTmpfsSize;
            "/tmp" = sizeBytes maxTmpfsBytes cfg.resources.tmpfsSize;
            "/var/tmp" = sizeBytes maxTmpfsBytes cfg.resources.tmpfsSize;
          }
        else
          "-";
      declaredPortsAbsent = if secure then bool ((container.ports or [ ]) == [ ]) else "-";
      declaredLiveResources =
        if
          secure
          && resourcesComplete container
          && declaredPidsLimit > 0
          && validSize maxMemoryBytes cfg.resources.memory
          && validSize maxMemoryBytes cfg.resources.memorySwap
          && validCpus cfg.resources.cpus
        then
          builtins.toJSON {
            memoryBytes = sizeBytes maxMemoryBytes cfg.resources.memory;
            memorySwapBytes = sizeBytes maxMemoryBytes cfg.resources.memorySwap;
            nanoCpus = cpuNano cfg.resources.cpus;
            pidsLimit = declaredPidsLimit;
            nofile = cfg.resources.nofile;
            nproc = declaredPidsLimit;
          }
        else
          "-";
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
        (bool (lib.all (mountSafe name piDurableRuntimeValid) (container.volumes or [ ])))
        (bool (piDurableRuntimeValid || researchRuntimeValid || ordinaryRuntimeValid))
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
        declaredRuntime
      ];
    in
    lib.concatStringsSep "\t" fields;
  securityManifest = lib.concatStringsSep "\n" (
    [
      "manifest\t2"
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
            validSize maxTmpfsBytes cfg.resources.tmpfsSize
            && validSize maxTmpfsBytes cfg.resources.runTmpfsSize;
          message = "tentaflake: secure tmpfs sizes must be positive byte sizes no greater than 64 GiB, with an optional K/M/G/T suffix.";
        }
      ]
      ++ lib.optionals secure [
        {
          assertion = agentContainers == { } || runtimes.stockSupported config;
          message = "tentaflake: secure agents require the exact backend-specific stock runsc registration without additional daemon runtime arguments.";
        }
        {
          assertion = runtimes.aliasesSupported config;
          message = "tentaflake: elevated gVisor runtime aliases, noncanonical registrations, and global defaults are forbidden.";
        }
      ]
      ++ lib.mapAttrsToList (name: container: {
        assertion = (runtimeValidity name container).allowed;
        message = "tentaflake: OCI container ${name} must retain its exact admitted runtime; direct paths, aliases and ungranted Pi Durable/Research transports cannot bypass agent admission.";
      }) (lib.optionalAttrs secure config.virtualisation.oci-containers.containers);

      warnings = lib.optional (cfg.profile == "dev") ''
        tentaflake security profile "dev" preserves compatibility and is not a
        security boundary for untrusted 24/7 agents.
      '';

      environment.etc."tentaflake/security.tsv".text = securityManifest + "\n";
    }

    (lib.mkIf secure {
      environment.systemPackages = [ pkgs.gvisor ];

      virtualisation = {
        docker.daemon.settings.runtimes.runsc.path = lib.getExe' pkgs.gvisor "runsc";
        podman.extraRuntimes = [ pkgs.gvisor ];
        containers.containersConf.settings.engine.runtimes.runsc = [ runtimes.stock.path ];
      };
    })
    (lib.mkIf (secure && piDurableTransportEnabled) {
      virtualisation.docker.daemon.settings.runtimes.runsc-pi-durable = {
        inherit (piDurableTransport) path;
      };
      # Pi Durable retains its separately admitted bind+connect runtime.
      virtualisation.containers.containersConf.settings.engine.runtimes.runsc-pi-durable = [
        piDurableTransport.path
      ];
    })
  ];
}
