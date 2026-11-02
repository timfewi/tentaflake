{ researchFlake }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.research;
  containers = config.virtualisation.oci-containers.containers;
  instances = config.tentaflake.agentInstances;
  adapters = import ../adapters { inherit pkgs lib; };
  secure = config.tentaflake.security.profile != "dev";
  transport = import ../lib/researchTransport.nix { inherit lib pkgs; };
in
{
  imports = [ researchFlake.nixosModules.default ];
  options.tentaflake.research = {
    clientPackage = lib.mkOption {
      type = lib.types.package;
      default = researchFlake.packages.${pkgs.stdenv.hostPlatform.system}.research-client;
      description = "Pinned stdio client of the isolated tentaflake-research service.";
    };
    agents = lib.mkOption {
      default = { };
      description = "Agents using secure-research-tool as their exclusive web/research interface. Configure the service and VPN explicitly; model calls remain on the LLM broker.";
      type = lib.types.attrsOf (
        lib.types.submodule {
          options.uid = lib.mkOption {
            type = lib.types.ints.between 1 4294967294;
            description = "Unique host relay UID; independent of the agent container UID.";
          };
        }
      );
    };
  };
  config = lib.mkIf (cfg.agents != { }) {
    services.secureResearch = {
      enable = lib.mkDefault true;
      containerClients = cfg.agents;
    };
    assertions = [
      {
        assertion = secure && config.services.secureResearch.enable;
        message = "Tentaflake research requires a secure profile and the isolated Research service.";
      }
      {
        assertion = transport.supported config;
        message = "Tentaflake Research requires its exact per-agent runtime registration, never an engine-wide default or extra runtime arguments.";
      }
      {
        assertion =
          config.services.secureResearch.summarizeOrder == [ ]
          && !(lib.attrByPath [ "openai" "enable" ] false config.services.secureResearch.providers);
        message = "Tentaflake Research must leave summarization disabled; model calls use the LLM broker.";
      }
    ]
    ++ lib.mapAttrsToList (
      name: _:
      let
        instance = instances.${name} or null;
        registered = instance != null && builtins.hasAttr instance.adapter adapters;
      in
      {
        assertion =
          builtins.hasAttr name containers
          && registered
          && instance.runnable
          && adapters.${instance.adapter}.research.supported
          && !(lib.attrByPath [ name "fetch" "enable" ] false config.tentaflake.broker.agents);
        message = "Research agent ${name} must be a declared registered adapter with Research capability and disable its legacy fetch broker.";
      }
    ) cfg.agents
    ++ lib.mapAttrsToList (
      name: _:
      let
        client = import ../lib/researchClient.nix {
          inherit config lib pkgs;
          containerName = name;
          settings = null;
        };
      in
      {
        assertion = lib.all (volume: lib.elem volume (containers.${name}.volumes or [ ])) client.volumes;
        message = "Research agent ${name} must retain its exact read-only client closure and relay socket mounts.";
      }
    ) cfg.agents
    ++ lib.mapAttrsToList (
      name: _:
      let
        instance = instances.${name} or null;
        expected =
          if instance != null && instance.adapter == "pi-durable" then "runsc-pi-durable" else transport.name;
      in
      {
        assertion =
          builtins.hasAttr name containers
          && transport.selected config.virtualisation.oci-containers.backend containers.${name} expected;
        message = "Research agent ${name} must explicitly select its dedicated scoped runtime.";
      }
    ) cfg.agents;
    # Stock runsc remains unchanged. Only Research-enabled agents select this
    # named connect-only transport; exact read-only mounts still bound access.
    virtualisation.docker.daemon.settings.runtimes.runsc-research.path = transport.path;
    virtualisation.containers.containersConf.settings.engine.runtimes.runsc-research = [
      transport.path
    ];
  };
}
