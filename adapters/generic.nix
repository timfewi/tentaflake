# Agent-independent OCI workload. Host policy owns the isolation backend.
{ pkgs, lib }:
let
  preset = {
    schemaVersion = 1;
    identity = {
      id = "generic";
      version = "1";
      status = "definition";
    };
    artifact = {
      kind = "instance";
      reference = null;
      reviewed = false;
    };
    command = [ ];
    configuration = {
      format = "none";
      readOnly = true;
    };
    ownership = {
      uid = 10000;
      gid = 10000;
    };
    layout = {
      state = "/var/lib/tentaflake/agents/<name>";
      workspace = "workspace";
      writable = [
        "state"
        "workspace"
      ];
      directories = [ ];
    };
    lifecycle = "service";
    model = {
      protocols = [ ];
      streaming = "unknown";
    };
    research = {
      supported = false;
    };
    execution = {
      interface = "worker-queue-v1";
      automatic = false;
    };
    capabilities = [
      "files"
      "shell"
      "terminal"
      "worker"
    ];
    evidence = {
      fixture = "source-regression";
      vendorAcceptance = "operator-owned";
    };
  };
  hostIdentity = import ../lib/agentIdentity.nix;
  validate = import ../lib/runtimeContract.nix { inherit lib; };
  security = import ../lib/containerSecurity.nix { inherit lib; };
  metadata =
    args: backend:
    let
      definition = validate args.definition;
      container = hostIdentity.container args.name;
      stateDir = hostIdentity.stateDir args.name;
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
  inherit metadata preset;
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
        worker = lib.attrByPath [ container ] null config.tentaflake.worker.agents;
        workerEnabled = worker != null && worker.enable;
        workerResults = "/var/lib/tentaflake-worker-${container}/results";
        quota = lib.attrByPath [ container ] null config.tentaflake.workspaceQuota.agents;
        quotaEnabled = quota != null && quota.enable;
        dependencies = lib.optional quotaEnabled "tentaflake-workspace-quota-${container}.service";
        result = security.apply {
          profile = config.tentaflake.security.profile;
          inherit backend owner;
          name = container;
          pidsLimit = d.resources.pidsLimit or 512;
          resources =
            config.tentaflake.security.resources // builtins.removeAttrs d.resources [ "pidsLimit" ];
          allowedWritableSources = [
            stateStorage
            workspace
          ];
          allowedWritableDestinations = [
            "/state"
            d.workspace
          ];
          approvedReadOnlySources = lib.optional workerEnabled workerResults;
          approvedReadOnlyDestinations = lib.optional workerEnabled "/run/tentaflake-worker/results";
          baseConfig = {
            inherit (d) image;
            cmd = d.command;
            autoStart = d.lifecycle == "service";
            user = owner;
            environment.HOME = "/state";
            volumes = [
              "${stateStorage}:/state:rw"
              "${workspace}:${d.workspace}:rw"
            ]
            ++ lib.optional workerEnabled "${workerResults}:/run/tentaflake-worker/results:ro";
            extraOptions = [ "--workdir=${d.workspace}" ];
          };
          # Admission stays fail-closed until capability-specific policy lands.
          brokerPolicyEnabled = false;
          researchPolicyEnabled = false;
          inherit workerEnabled;
          workspaceQuotaEnabled = quotaEnabled;
          automaticStart = config.virtualisation.oci-containers.containers.${container}.autoStart;
        };
      in
      {
        assertions = result.assertions ++ [
          {
            assertion = config.tentaflake.security.profile != "dev";
            message = "tentaflake: generic workloads require the common secure runtime; dev is legacy compatibility only.";
          }
          {
            assertion =
              !workerEnabled
              || (
                lib.elem "worker" d.capabilities
                && worker.workspace == workspace
                && worker.containerUid == uid
                && worker.containerGid == gid
              );
            message = "tentaflake: generic worker declarations must match requested capability, workspace and ownership.";
          }
          {
            assertion = !quotaEnabled || quota.workspace == workspace;
            message = "tentaflake: generic workspace quota must match the isolated workspace.";
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
