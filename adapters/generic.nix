# Agent-independent OCI workload. Host policy owns the isolation backend.
{ pkgs, lib }:
let
  preset = (import ../lib/runtimeCatalog.nix).presets.generic;
  validate = import ../lib/runtimeContract.nix { inherit lib; };
  security = import ../lib/containerSecurity.nix { inherit lib; };
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
