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
  instances = lib.genAttrs instanceNames instance;
  baseImages = import ../lib/baseImages.nix { inherit lib; };
  quotaHelpers = import ../lib/quotaHelpers.nix { inherit pkgs lib; };
  templateId = lib.types.strMatching "[A-Za-z0-9][A-Za-z0-9_.-]{0,95}";
  issuedActions = [
    "environments.read"
    "workspaces.read"
    "workspaces.retain"
  ];
  creationTemplate =
    id: entry:
    let
      source = instances.${entry.sourceInstance} or null;
      metadata = config.tentaflake.agentInstances.${entry.sourceInstance} or null;
      spec = runtimes.${entry.sourceInstance} or null;
      owned = lib.filter (volume: volume.spec.instanceId == entry.sourceInstance) volumes;
      image = baseImages.catalog.images.${entry.imageId};
      platform = image.platforms.${pkgs.stdenv.hostPlatform.system};
      reference = baseImages.image {
        id = entry.imageId;
        system = pkgs.stdenv.hostPlatform.system;
      };
      body = {
        inherit (entry) sourceInstance;
        sourceRevision = source.revision;
        inherit reference;
        image = {
          id = entry.imageId;
          inherit (image) distro release label;
          digest = platform.manifestDigest;
        };
        inherit (entry) principals issuedActions;
        helpers = builtins.listToAttrs (
          map (
            volume:
            lib.nameValuePair volume.spec.kind (
              lib.mapAttrs (_: toString) (quotaHelpers {
                name = "template-${id}";
                inherit (volume.spec) kind ownerUid ownerGid;
                bytes = volume.spec.bytes;
                stateDirectories = lib.optionals (volume.spec.kind == "state") metadata.stateDirectories;
                dynamic = true;
              })
            )
          ) owned
        );
      };
      mib = 1024 * 1024;
    in
    assert lib.assertMsg
      (
        builtins.match "[A-Za-z0-9][A-Za-z0-9_.-]{0,95}" id != null
        && source != null
        && metadata != null
        && spec != null
        && metadata.adapter == "generic"
        && metadata.requestedCapabilities == [ ]
        && source.isolation == "container"
        && spec.image.kind == "registry"
        && spec.image.reference == reference
        && spec.provenance == null
        && !spec.container.autoStart
        && spec.container.dependsOn == [ ]
        && spec.container.hostname == null
        && spec.container.networks == [ ]
        && spec.container.environmentFiles == [ ]
        && builtins.length owned == 2
        &&
          lib.sort builtins.lessThan (map (volume: volume.spec.kind) owned) == [
            "state"
            "workspace"
          ]
        && builtins.length spec.container.volumes == 2
        && lib.all (
          volume:
          lib.any (
            mount: lib.hasPrefix "${volume.spec.mountPath}:" mount && lib.hasSuffix ":rw" mount
          ) spec.container.volumes
        ) owned
        && source.resources.nanoCpus >= 1000000000
        && source.resources.nanoCpus <= 1024000000000
        && lib.mod source.resources.nanoCpus 1000000000 == 0
        && lib.all (bytes: bytes >= 128 * mib && bytes <= 1048576 * mib && lib.mod bytes mib == 0) [
          source.resources.memoryBytes
          source.resources.workspaceBytes
        ]
        && entry.principals != [ ]
        && builtins.length entry.principals <= 128
        && lib.unique entry.principals == entry.principals
        && entry.issuedActions != [ ]
        && lib.unique entry.issuedActions == entry.issuedActions
      )
      "tentaflake creation template ${id} requires an exact reviewed offline Generic source with two quotas and representable fixed resources.";
    body // { revision = revision body; };
  creationTemplates = lib.mapAttrs creationTemplate cfg.creationTemplates;
  templateSignature =
    template:
    let
      resources = instances.${template.sourceInstance}.resources;
    in
    builtins.toJSON {
      image = template.image.id;
      cpu = resources.nanoCpus;
      memory = resources.memoryBytes;
      storage = resources.workspaceBytes;
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
    inherit instances creationTemplates;
    creationCatalog = if creationTemplates == { } then null else "${../images/catalog.json}";
    inherit volumes runtimes;
    inherit (config.tentaflake.worker.maintenance) workers;
    workerImage = config.tentaflake.worker.maintenance.image;
    apiConfig = config.tentaflake.api.configuration;
    tools = {
      cli = "${pkgs.callPackage ../pkgs/tentaflake-cli { }}/bin/tentaflake";
      systemctl = "${pkgs.systemd}/bin/systemctl";
      busctl = "${pkgs.systemd}/bin/busctl";
      findmnt = "${pkgs.util-linux}/bin/findmnt";
      losetup = "${pkgs.util-linux}/bin/losetup";
      blkid = "${pkgs.util-linux}/bin/blkid";
      btrfs = "${pkgs.btrfs-progs}/bin/btrfs";
      docker = "${config.virtualisation.docker.package}/bin/docker";
      nixStore = "${config.nix.package}/bin/nix-store";
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
            creationTemplates = lib.mkOption {
              default = { };
              description = "Explicit private stopped-environment templates; declarations do not enable HTTP creation or authorize helpers.";
              type = lib.types.attrsOf (
                lib.types.submodule {
                  options = {
                    sourceInstance = lib.mkOption { type = templateId; };
                    imageId = lib.mkOption { type = lib.types.enum (lib.attrNames baseImages.catalog.images); };
                    principals = lib.mkOption { type = lib.types.listOf templateId; };
                    issuedActions = lib.mkOption { type = lib.types.listOf (lib.types.enum issuedActions); };
                  };
                }
              );
            };
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
          assertion =
            let
              signatures = map templateSignature (lib.attrValues creationTemplates);
            in
            builtins.length signatures <= 128 && lib.unique signatures == signatures;
          message = "tentaflake creation templates must have unique public configurations and a bounded inventory.";
        }
        {
          assertion =
            creationTemplates == { }
            || (
              lib.all (name: builtins.match "env-[0-9a-f]{32}" name == null) instanceNames
              && lib.all (
                volume:
                let
                  path = volume.spec.mountPath;
                  root = "/var/lib/tentaflake-environments";
                in
                path != root && !(lib.hasPrefix "${root}/" path) && !(lib.hasPrefix "${path}/" root)
              ) volumes
            );
          message = "tentaflake creation templates reserve their generated slot and mount namespace.";
        }
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
          assertion = !config.virtualisation.docker.daemon.settings.features.containerd-snapshotter;
          message = "tentaflake host admission requires the classic Docker image store (features.containerd-snapshotter=false) for exact config-digest identity; changing an existing store requires operator-owned migration.";
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

    virtualisation.docker.daemon.settings = lib.mkIf (cfg != null) {
      live-restore = lib.mkDefault false;
      features.containerd-snapshotter = lib.mkDefault false;
    };

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
        "tentaflake-environment-create@" = lib.mkIf (creationTemplates != { }) {
          description = "Provision one admitted stopped environment";
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = false;
            Restart = "no";
            KillMode = "control-group";
            TimeoutStartSec = 300;
            TimeoutStopSec = 120;
            UMask = "0077";
            ExecStart = lib.escapeShellArgs [
              manifest.tools.cli
              "admission"
              "create-execute"
              (toString config.tentaflake.admissionManifest)
              "%i"
            ];
          };
        };
        # NixOS defines this service even when its timer is disabled. Mask it
        # so bootstrap can prove it cannot mutate the stopped runtime inventory.
        docker-prune.enable = lib.mkForce false;
      }
    );
  };
}
