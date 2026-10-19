# Investigated upstream v2026.9.7. This is deliberately a stopped scaffold:
# no reviewed runtime artifact or accepted model/Research/execution routing.
# See docs/agent-adapters.md before adding an OCI image or enabling startup.
{ lib, ... }:
let
  preset = (import ../lib/runtimeCatalog.nix).presets.openclaw;
  reason = "OpenClaw is a stopped scaffold. The LLM broker supports opt-in streaming, but acceptance with the pinned OpenClaw executable is unverified. A reviewed image digest, accepted model transport, actual Research discovery, and disposable-worker tool routing require acceptance evidence. See docs/agent-adapters.md.";
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
      stateDir ? "/var/lib/openclaw-${name}",
      settings ? { },
    }:
    if autoStart then
      throw "tentaflake: ${reason} Keep autoStart = false."
    else if settings != { } then
      throw "tentaflake: OpenClaw settings are unavailable until the pinned upstream parser and tool controls pass acceptance."
    else if builtins.match "/var/lib/[a-zA-Z0-9_-]+(/[a-zA-Z0-9_-]+)*" stateDir == null then
      throw "tentaflake: OpenClaw scaffold stateDir must be a normalized directory below /var/lib."
    else
      { config, ... }: {
        # No OCI container or image pull is created. An explicit start fails loudly.
        systemd.services."${config.virtualisation.oci-containers.backend}-openclaw-${name}" = {
          description = "Stopped OpenClaw scaffold ${name} (runtime acceptance pending)";
          serviceConfig = {
            Type = "oneshot";
            Restart = "no";
          };
          script = ''
            echo ${lib.escapeShellArg reason} >&2
            exit 1
          '';
        };
        systemd.tmpfiles.rules = [
          "d ${stateDir} 0700 1000 1000 -"
          "d ${stateDir}/workspace 0700 1000 1000 -"
        ];
      };
  metadata =
    args: backend:
    let
      stateDir = args.stateDir or "/var/lib/openclaw-${args.name}";
      container = "openclaw-${args.name}";
    in
    {
      adapter = "openclaw";
      inherit (args) name;
      inherit container stateDir;
      stateStorage = stateDir;
      unit = "${backend}-${container}.service";
      workspace = "${stateDir}/workspace";
      uid = 1000;
      gid = 1000;
      runnable = false;
    };
}
