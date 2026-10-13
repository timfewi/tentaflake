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
  revision = value: "sha256:${builtins.hashString "sha256" (builtins.toJSON value)}";
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
    definition // { revision = revision definition; };
  plan = {
    schemaVersion = 1;
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
    inherit volumes;
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
    assertions = lib.optionals (cfg != null) [
      {
        assertion = security.profile == "balanced";
        message = "tentaflake host admission currently requires the balanced container profile.";
      }
      {
        assertion = config.virtualisation.oci-containers.backend == "docker";
        message = "tentaflake host admission currently requires Docker evidence; other backends are not inferred ready.";
      }
    ];
  };
}
