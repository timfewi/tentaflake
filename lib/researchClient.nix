# Reuse the isolated service; the agent receives only its socket capability.
{
  config,
  lib,
  pkgs,
  containerName,
  settings,
  hermes ? false,
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
  policy =
    if hermes then
      {
        agent.disabled_toolsets = lib.unique (
          (original.agent.disabled_toolsets or [ ])
          ++ [
            "web"
            "browser"
          ]
        );
        mcp_servers.secure-research-tool = server // {
          keepalive_interval = 20;
        };
      }
    else
      {
        browser.enabled = false;
        http_request.enabled = false;
        web_fetch.enabled = false;
        web_search.enabled = false;
        mcp = {
          enabled = true;
          servers = [
            (
              server
              // {
                name = "secure-research-tool";
                transport = "stdio";
              }
            )
          ];
        };
      };
in
{
  inherit enabled paths;
  assertions = lib.optionals (enabled && hermes) [
    {
      assertion = lib.all (entry: builtins.isAttrs entry && entry ? command && !(entry ? url)) (
        lib.attrValues (original.mcp_servers or { })
      );
      message = "tentaflake: Research agent ${containerName} may add only local stdio MCP servers; remote network tools are disabled.";
    }
  ];
  settings = if enabled then lib.recursiveUpdate original policy else settings;
  volumes =
    map (path: "${path}:${path}:ro") paths ++ lib.optional enabled "${directory}:${destination}:ro";
  readOnlySources = lib.optional enabled directory;
  readOnlyDestinations = lib.optional enabled destination;
  units = lib.optional enabled "tentaflake-research-${containerName}.socket";
}
