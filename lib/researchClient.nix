# Reuse the isolated service; the agent receives only its socket capability.
{
  config,
  lib,
  pkgs,
  containerName,
  settings,
  adapter ? null,
}:
let
  cfg = lib.attrByPath [ "tentaflake" "research" ] { agents = { }; } config;
  enabled = builtins.hasAttr containerName cfg.agents;
  directory = "/run/tentaflake-research/${containerName}";
  destination = "/run/tentaflake-research";
  paths =
    if enabled then
      lib.splitString "\n" (
        lib.removeSuffix "\n" (
          builtins.readFile "${pkgs.closureInfo { rootPaths = [ cfg.clientPackage ]; }}/store-paths"
        )
      )
    else
      [ ];
  server = {
    command = "${cfg.clientPackage}/bin/research-client";
    args = [
      "--socket"
      "${destination}/socket"
    ];
  };
  original = if settings == null then { } else settings;
  registry = import ../adapters { inherit pkgs lib; };
  integration =
    if adapter == null then
      {
        supported = false;
        configure = _: {
          settings = { };
          valid = true;
        };
      }
    else if builtins.hasAttr adapter registry then
      registry.${adapter}.research
    else
      throw "tentaflake: unknown Research adapter ${adapter}.";
  projection = integration.configure { inherit original server; };

in
{
  inherit enabled paths;
  assertions = lib.optionals (enabled && adapter != null) [
    {
      assertion = integration.supported;
      message = "tentaflake: Research adapter ${adapter} for ${containerName} has no accepted tool discovery integration.";
    }
    {
      assertion = projection.valid;
      message = "tentaflake: Research agent ${containerName} may add only local stdio MCP servers; remote network tools are disabled.";
    }
  ];
  settings = if enabled then lib.recursiveUpdate original projection.settings else settings;
  volumes =
    map (path: "${path}:${path}:ro") paths ++ lib.optional enabled "${directory}:${destination}:ro";
  readOnlySources = lib.optional enabled directory;
  readOnlyDestinations = lib.optional enabled destination;
  units = lib.optional enabled "tentaflake-research-${containerName}.socket";
}
