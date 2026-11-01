# Shared stopped-scaffold adapter for runtimes without acceptance evidence.
# It reserves the instance identity and an explicit refusal unit only: no
# container, image, state directory, credential or socket is created.
{ lib }:
{
  id,
  preset,
  displayName,
  reason,
  settingsReason,
}:
let
  hostIdentity = import ./agentIdentity.nix;
  defaultStateDir = hostIdentity.stateDir;
in
{
  inherit preset;
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
    generate = _: _: throw "tentaflake: ${reason}";
  };
  research = preset.research // {
    configure = _: {
      settings = { };
      valid = false;
    };
  };
  build =
    {
      name,
      autoStart ? false,
      stateDir ? defaultStateDir name,
      settings ? { },
    }:
    let
      container = hostIdentity.container name;
    in
    if autoStart then
      throw "tentaflake: ${reason} Keep autoStart = false."
    else if settings != { } then
      throw "tentaflake: ${settingsReason}"
    else if
      !builtins.isString stateDir
      || builtins.match "/var/lib/[a-zA-Z0-9_-]+(/[a-zA-Z0-9_-]+)*" stateDir == null
    then
      throw "tentaflake: ${displayName} scaffold stateDir must be a normalized directory below /var/lib."
    else
      { config, ... }: {
        assertions = [
          {
            assertion =
              !(builtins.hasAttr container (
                lib.attrByPath [ "virtualisation" "oci-containers" "containers" ] { } config
              ));
            message = "tentaflake: ${displayName} scaffold ${container} cannot share its identity with an OCI container.";
          }
          {
            assertion =
              !(lib.attrByPath [ "tentaflake" "workspaceQuota" "agents" container "enable" ] false config);
            message = "tentaflake: ${displayName} scaffold ${container} cannot provision workspace or state quotas.";
          }
        ];
        # An explicit start fails loudly.
        systemd.services."${config.virtualisation.oci-containers.backend}-${container}" = {
          description = "Stopped ${displayName} scaffold ${name} (runtime acceptance pending)";
          wantedBy = [ ];
          serviceConfig = {
            Type = "oneshot";
            Restart = "no";
          };
          script = ''
            echo ${lib.escapeShellArg reason} >&2
            exit 1
          '';
        };
      };
  metadata =
    args: backend:
    let
      stateDir = args.stateDir or (defaultStateDir args.name);
      container = hostIdentity.container args.name;
    in
    {
      adapter = id;
      inherit (args) name;
      inherit container stateDir;
      stateStorage = stateDir;
      unit = "${backend}-${container}.service";
      workspace = "${stateDir}/workspace";
      inherit (preset.ownership) uid gid;
      runnable = false;
    };
}
