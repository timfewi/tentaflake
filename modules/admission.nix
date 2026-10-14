{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.security.admission;
  limits = import ../lib/resourceLimits.nix { inherit lib; };
  security = config.tentaflake.security;
  resourceFields = [
    "memoryBytes"
    "memorySwapBytes"
    "nanoCpus"
    "pids"
    "workspaceBytes"
    "stateBytes"
  ];
  resourcesType = lib.types.submodule {
    options = lib.genAttrs resourceFields (_: lib.mkOption { type = lib.types.ints.unsigned; });
  };
  zeroResources = lib.genAttrs resourceFields (_: 0);
  volumes = lib.attrValues config.tentaflake.admissionVolumes;
  instanceNames = lib.unique (map (volume: volume.spec.instanceId) volumes);
  runtimeNames = lib.filter (name: lib.hasAttr name config.tentaflake.agentInstances) instanceNames;
  containers = config.virtualisation.oci-containers.containers;
  revision = value: "sha256:${builtins.hashString "sha256" (builtins.toJSON value)}";
  runtime =
    name:
    let
      container = containers.${name};
      declaredImage = config.tentaflake.agentInstances.${name}.runtimeImage;
      provenance = config.tentaflake.imageProvenance.agents.${name} or null;
    in
    {
      image =
        if declaredImage != null then
          declaredImage
        else
          {
            kind = "registry";
            reference = container.image;
          };
      unit = "${container.serviceName}.service";
      # Project the complete final OCI input, not a second containment policy.
      # These removed source/backend fields are either represented by `image`
      # or explicitly forbidden below. Do not read rendered systemd commands:
      # those commands depend on this manifest and would introduce a cycle.
      container = builtins.removeAttrs container [
        "image"
        "imageFile"
        "imageStream"
        "podman"
      ];
      provenance = if provenance != null && provenance.enable then provenance else null;
    };
  runtimes = lib.genAttrs runtimeNames runtime;
  instance =
    name:
    let
      owned = lib.filter (volume: volume.spec.instanceId == name) volumes;
      metadata = config.tentaflake.agentInstances.${name} or null;
      container = config.virtualisation.oci-containers.containers.${name};
      runtimeResources = limits.ociResources container.extraOptions;
      storageBytes =
        kind:
        lib.foldl' (sum: volume: sum + (if volume.spec.kind == kind then volume.spec.bytes else 0)) 0 owned;
      definition = {
        id = name;
        ownerId = "host-root";
        isolation = if security.profile == "strict" then "guest-kernel" else "container";
        resources =
          zeroResources
          // {
            workspaceBytes = storageBytes "workspace";
            stateBytes = storageBytes "state";
          }
          // lib.optionalAttrs (runtimeResources != null) {
            inherit (runtimeResources) memoryBytes memorySwapBytes nanoCpus;
            pids = runtimeResources.pidsLimit;
          };
        requestedCapabilities = lib.optionals (metadata != null) (
          lib.filter (
            capability:
            lib.elem capability [
              "model"
              "research"
              "worker"
            ]
          ) metadata.requestedCapabilities
        );
        storage = map (volume: {
          id = volume.spec.allocationId;
          inherit (volume.spec) kind bindingRevision;
        }) owned;
      };
    in
    definition
    // {
      revision = revision {
        inherit definition;
        runtime = runtimes.${name} or null;
      };
    };
  plan = {
    schemaVersion = 3;
    inherit (cfg) physicalSpaceHeadroomBytes;
    policy = {
      generation = cfg.authorityEpoch;
      inherit (cfg)
        capacity
        hostReserve
        researchReserve
        maxActive
        evidenceMaxAgeSeconds
        ;
      perInstance = {
        memoryBytes = limits.sizeBytes limits.maxMemoryBytes security.resources.memory;
        memorySwapBytes = limits.sizeBytes limits.maxMemoryBytes security.resources.memorySwap;
        nanoCpus = limits.cpuNano security.resources.cpus;
        pids = security.resources.pidsLimit;
        workspaceBytes = 1024 * 1024 * 1024 * 1024;
        stateBytes = 1024 * 1024 * 1024 * 1024;
      };
      grants = [ ];
    };
    instances = lib.genAttrs instanceNames instance;
    inherit volumes runtimes;
    inherit (config.tentaflake.worker.maintenance) workers;
    workerImage = config.tentaflake.worker.maintenance.image;
    tools = {
      cli = "${pkgs.callPackage ../pkgs/tentaflake-cli { }}/bin/tentaflake";
      systemctl = "${pkgs.systemd}/bin/systemctl";
      busctl = "${pkgs.systemd}/bin/busctl";
      findmnt = "${pkgs.util-linux}/bin/findmnt";
      losetup = "${pkgs.util-linux}/bin/losetup";
      blkid = "${pkgs.util-linux}/bin/blkid";
      btrfs = "${pkgs.btrfs-progs}/bin/btrfs";
      docker = "${config.virtualisation.docker.package}/bin/docker";
    };
  };
  manifest = plan // {
    policy = plan.policy // {
      revision = revision plan;
    };
  };
in
{
  options.tentaflake = {
    security.admission = lib.mkOption {
      default = null;
      description = ''
        Explicit host admission capacity and reserves. Initial inventory and policy
        publication are operator operations; a missing ledger never means empty.
        Legacy controllers, independent workers and other unmanaged workloads must
        fit the host reserve. These declarations are not live readiness evidence.
      '';
      type = lib.types.nullOr (
        lib.types.submodule {
          options = {
            authorityEpoch = lib.mkOption {
              type = lib.types.strMatching "[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}";
              description = "Durable host authority identity; never rotate it to reset existing claims.";
            };
            capacity = lib.mkOption { type = resourcesType; };
            hostReserve = lib.mkOption { type = resourcesType; };
            researchReserve = lib.mkOption { type = resourcesType; };
            maxActive = lib.mkOption { type = lib.types.ints.between 1 4096; };
            evidenceMaxAgeSeconds = lib.mkOption {
              type = lib.types.ints.between 1 300;
              default = 5;
            };
            physicalSpaceHeadroomBytes = lib.mkOption {
              type = lib.types.ints.unsigned;
              description = ''
                Required free-byte headroom at storage admission, in addition to
                conservative outstanding shared-pool image charges. This is a
                live admission-time guard, not a physical-space reservation.
              '';
            };
          };
        }
      );
    };
    admissionManifest = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      readOnly = true;
      internal = true;
      description = "Immutable generated host admission plan; contains no live evidence or credentials.";
    };
  };
  config = {
    tentaflake.admissionManifest =
      if cfg == null then null else pkgs.writeText "tentaflake-admission.json" (builtins.toJSON manifest);
    environment.etc = lib.mkIf (cfg != null) {
      "tentaflake/admission.json".source = config.tentaflake.admissionManifest;
    };
    assertions =
      lib.optionals (cfg != null) [
        {
          assertion = security.profile == "balanced";
          message = "tentaflake host admission currently requires the balanced container profile.";
        }
        {
          assertion = config.virtualisation.oci-containers.backend == "docker";
          message = "tentaflake host admission currently requires Docker evidence; other backends are not inferred ready.";
        }
        {
          assertion = !config.virtualisation.docker.daemon.settings.live-restore;
          message = "tentaflake host admission requires Docker live-restore disabled so explicit stopped-daemon bootstrap can fence legacy dispatch.";
        }
        {
          assertion = !config.virtualisation.docker.autoPrune.enable;
          message = "tentaflake host admission requires Docker automatic pruning disabled to preserve exact retained container identities.";
        }
      ]
      ++ lib.optionals (cfg != null) (
        map (name: {
          assertion =
            let
              container = containers.${name};
            in
            container.serviceName == "docker-${name}"
            && name != "prune"
            && container.imageFile == null
            && container.imageStream == null
            && container.podman == null
            && lib.all (value: value == null) (lib.attrValues container.login)
            && container.preRunExtraOptions == [ ]
            && container.pull == "never"
            && !container.autoRemoveOnStop;
          message = "tentaflake admitted runtime ${name} requires the owned Docker supervisor, separately acquired images, and retained exact container identities.";
        }) runtimeNames
      );

    virtualisation.docker.daemon.settings.live-restore = lib.mkIf (cfg != null) (lib.mkDefault false);

    virtualisation.oci-containers.containers = lib.mkIf (cfg != null) (
      lib.genAttrs runtimeNames (_: {
        pull = lib.mkDefault "never";
        autoRemoveOnStop = lib.mkDefault false;
      })
    );

    systemd.services = lib.mkIf (cfg != null) (
      lib.genAttrs (map (name: "docker-${name}") runtimeNames) (
        unit:
        let
          name = lib.removePrefix "docker-" unit;
          command =
            action:
            lib.escapeShellArgs [
              manifest.tools.cli
              "admission"
              action
              (toString config.tentaflake.admissionManifest)
              name
            ];
        in
        {
          # Existing quota, capability and provenance Requires/After edges stay
          # attached to this unit. Acquisition is never a start hook.
          script = lib.mkForce "";
          preStart = lib.mkForce "";
          postStart = lib.mkForce "";
          preStop = lib.mkForce "";
          postStop = lib.mkForce "";
          reload = lib.mkForce "";
          serviceConfig = {
            Type = lib.mkForce "notify";
            NotifyAccess = lib.mkForce "main";
            RemainAfterExit = lib.mkForce false;
            Restart = lib.mkForce "no";
            KillMode = lib.mkForce "control-group";
            TimeoutStartSec = lib.mkForce 300;
            TimeoutStopSec = lib.mkForce 120;
            ExecCondition = lib.mkForce [ ];
            ExecStartPre = lib.mkForce [ ];
            ExecStart = lib.mkForce (command "runtime-start");
            ExecStartPost = lib.mkForce [ ];
            ExecStop = lib.mkForce (command "runtime-stop");
            ExecStopPost = lib.mkForce [ ];
            ExecReload = lib.mkForce [ ];
          };
        }
      )
      // {
        # NixOS defines this service even when its timer is disabled. Mask it
        # so bootstrap can prove it cannot mutate the stopped runtime inventory.
        docker-prune.enable = lib.mkForce false;
      }
    );
  };
}
