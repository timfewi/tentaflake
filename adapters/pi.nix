# Pi Durable is deliberately a stopped scaffold: no reviewed runtime image,
# accepted runner, host socket boundary or live provider admission exists.
{ lib, ... }:
let
  preset = (import ../lib/runtimeCatalog.nix).presets.pi;
  reason = "Pi is a stopped scaffold. A reviewed image, runner, Board and Research socket boundaries, broker compatibility, and lifecycle acceptance evidence are required before admission. See docs/agent-adapters.md.";
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
      stateDir ? "/var/lib/pi-${name}",
      settings ? { },
    }:
    if autoStart then
      throw "tentaflake: ${reason} Keep autoStart = false."
    else if settings != { } then
      throw "tentaflake: Pi settings are unavailable until runtime acceptance."
    else if
      !builtins.isString stateDir
      || builtins.match "/var/lib/[a-zA-Z0-9_-]+(/[a-zA-Z0-9_-]+)*" stateDir == null
    then
      throw "tentaflake: Pi scaffold stateDir must be a normalized directory below /var/lib."
    else
      { config, ... }: {
        assertions = [
          {
            assertion =
              !(builtins.hasAttr "pi-${name}" (
                lib.attrByPath [ "virtualisation" "oci-containers" "containers" ] { } config
              ));
            message = "tentaflake: Pi scaffold pi-${name} cannot share its identity with an OCI container.";
          }
          {
            assertion =
              !(lib.attrByPath [ "tentaflake" "workspaceQuota" "agents" "pi-${name}" "enable" ] false config);
            message = "tentaflake: Pi scaffold pi-${name} cannot provision workspace or state quotas.";
          }
        ];
        # No container, image, state directory, credential or socket is created.
        systemd.services."${config.virtualisation.oci-containers.backend}-pi-${name}" = {
          description = "Stopped Pi scaffold ${name} (runtime acceptance pending)";
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
      stateDir = args.stateDir or "/var/lib/pi-${args.name}";
      container = "pi-${args.name}";
    in
    {
      adapter = "pi";
      inherit (args) name;
      inherit container stateDir;
      stateStorage = stateDir;
      unit = "${backend}-${container}.service";
      workspace = "${stateDir}/workspace";
      inherit (preset.ownership) uid gid;
      runnable = false;
    };
}
