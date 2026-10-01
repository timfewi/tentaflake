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
  secure = config.tentaflake.security.profile != "dev";
  runsc = pkgs.writeShellScriptBin "runsc" ''
    exec ${lib.getExe' pkgs.gvisor "runsc"} --host-uds=open "$@"
  '';
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
        assertion =
          config.services.secureResearch.summarizeOrder == [ ]
          && !(lib.attrByPath [ "openai" "enable" ] false config.services.secureResearch.providers);
        message = "Tentaflake Research must leave summarization disabled; model calls use the LLM broker.";
      }
    ]
    ++ lib.mapAttrsToList (name: _: {
      assertion =
        builtins.hasAttr name containers
        && (lib.hasPrefix "hermes-" name || lib.hasPrefix "zeroclaw-" name)
        && !(lib.attrByPath [ name "fetch" "enable" ] false config.tentaflake.broker.agents);
      message = "Research agent ${name} must be a declared Hermes/ZeroClaw container and disable its legacy fetch broker.";
    }) cfg.agents
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
    ) cfg.agents;
    # gVisor defaults to denying host UDS. Permit opening projected sockets;
    # creation remains disabled, and mount policy admits only each exact relay.
    virtualisation.docker.daemon.settings.runtimes.runsc.path = lib.mkForce (lib.getExe runsc);
    virtualisation.podman.extraRuntimes = lib.mkForce [ runsc ];
  };
}
