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
  runsc = pkgs.writeShellScriptBin "runsc" ''
    exec ${lib.getExe' pkgs.gvisor "runsc"} --host-uds=open "$@"
  '';
  research = config.services.secureResearch;
  controlPath = "/run/agent-research-egress/control/state.json";
  configDestination = "/etc/research/config.json";
  admissionAgents = lib.filterAttrs (
    name: _:
    let
      instance = instances.${name} or null;
    in
    instance != null
    && instance.adapter == "generic"
    && lib.elem "research" instance.requestedCapabilities
  ) cfg.agents;
  storePath =
    path:
    builtins.isString path
    &&
      builtins.match "/nix/store/[0-9abcdfghijklmnpqrsvwxyz]{32}-[A-Za-z0-9+._=-]+(/[A-Za-z0-9+._=-]+)*" path
      != null
    && lib.all (part: part != "." && part != "..") (lib.splitString "/" path);
  # Project the final unit, but accept only the upstream's canonical command and
  # mount grammar. This is an identity contract, not live readiness evidence.
  process =
    {
      name,
      argv,
      uid,
      privateNetwork,
      runtimePaths,
      hasConfig ? false,
      writablePaths ? [ ],
    }:
    let
      final = config.systemd.services.${name}.serviceConfig;
      bindings = final.BindReadOnlyPaths or [ ];
      configBindings = lib.filter (binding: lib.hasSuffix ":${configDestination}" binding) bindings;
      configuration =
        if hasConfig && lib.length configBindings == 1 then
          {
            source = lib.removeSuffix ":${configDestination}" (builtins.head configBindings);
            destination = configDestination;
          }
        else
          null;
      finalUid = config.users.users.${final.User or "root"}.uid or null;
      rootDirectory = toString (final.RootDirectory or "");
    in
    {
      spec = {
        unit = "${name}.service";
        executable = builtins.head argv;
        inherit argv rootDirectory configuration;
        uid = finalUid;
        slice = final.Slice or "";
        bindReadOnlyPaths = bindings;
      };
      valid =
        (final.ExecStart or null) == lib.concatStringsSep " " argv
        && finalUid == uid
        && builtins.isInt finalUid
        && finalUid >= 0
        && finalUid < 4294967295
        && storePath rootDirectory
        && storePath (builtins.head argv)
        && (final.Slice or "") == "agent-research.slice"
        && (final.PrivateNetwork or false) == privateNetwork
        && !(final.PrivateUsers or false)
        && !(final.DynamicUser or false)
        && (final.KillMode or "control-group") == "control-group"
        && (final.BindPaths or [ ]) == writablePaths
        && lib.length bindings == lib.length (lib.unique bindings)
        && (
          if hasConfig then configuration != null && storePath configuration.source else configBindings == [ ]
        )
        && lib.all (
          binding:
          storePath binding
          || lib.elem binding runtimePaths
          || (configuration != null && binding == "${configuration.source}:${configDestination}")
        ) bindings
        && lib.all (path: lib.elem path bindings) runtimePaths;
    };
  sharedProcesses = {
    service = process {
      name = "agent-research";
      argv = [
        "${research.package}/bin/research-service"
        "--listen-fd"
        "3"
        "--temporary-directory"
        "/tmp/research"
        "--public-only"
        "--config"
        configDestination
      ];
      uid = research.serviceUid;
      privateNetwork = true;
      runtimePaths = [
        "/run/agent-research"
        "/run/agent-research-egress"
      ]
      ++ lib.optional research.searxng.enable "/run/agent-research-searxng";
      hasConfig = true;
    };
    egress = process {
      name = "agent-research-egress";
      argv = [
        "${research.egressPackage}/bin/research-egress"
        "--listen-fd"
        "3"
        "--config"
        configDestination
      ];
      uid = research.egressUid;
      privateNetwork = false;
      runtimePaths = [ "/run/agent-research-egress" ];
      hasConfig = true;
    };
    controller = process {
      name = "agent-research-egress-control";
      argv = [
        "${research.egressPackage}/bin/research-egress-control"
        "--input"
        research.observationFile
        "--output"
        controlPath
        "--drain-seconds"
        (toString research.drainSeconds)
      ];
      uid = 0;
      privateNetwork = true;
      runtimePaths = [ (builtins.dirOf research.observationFile) ];
      writablePaths = [ (builtins.dirOf controlPath) ];
    };
  };
  admissionProjections = lib.mapAttrs (
    name: agent:
    let
      client = import ../lib/researchClient.nix {
        inherit config lib pkgs;
        containerName = name;
        settings = null;
      };
      relayName = "tentaflake-research-${name}";
      socket = config.systemd.sockets.${relayName};
      socketPath = "/run/tentaflake-research/${name}/socket";
      relay = process {
        name = "${relayName}@";
        argv = [
          "${pkgs.socat}/bin/socat"
          "STDIO"
          "UNIX-CONNECT:/run/agent-research/socket"
        ];
        inherit (agent) uid;
        privateNetwork = true;
        runtimePaths = [ "/run/agent-research" ];
      };
    in
    {
      spec = {
        schemaVersion = 1;
        inherit controlPath;
        inherit (research) vpnInterface;
        client = {
          executable = client.server.command;
          closurePaths = client.paths;
          socketDestination = builtins.elemAt client.server.args 1;
        };
        relay = {
          socketUnit = "${relayName}.socket";
          inherit socketPath;
          template = relay.spec;
        };
      }
      // lib.mapAttrs (_: projected: projected.spec) sharedProcesses;
      valid =
        relay.valid
        && lib.all (projected: projected.valid) (lib.attrValues sharedProcesses)
        && socket.listenStreams == [ socketPath ]
        && lib.filterAttrs (key: _: lib.hasPrefix "Listen" key) socket.socketConfig == { }
        && (socket.socketConfig.Service or "${relayName}@.service") == "${relayName}@.service"
        && socket.socketConfig.Accept
        && socket.socketConfig.SocketUser == "root"
        && socket.socketConfig.SocketGroup == "root"
        && socket.socketConfig.SocketMode == "0666"
        && socket.socketConfig.MaxConnections == 4
        && socket.socketConfig.RemoveOnStop
        && config.systemd.services."${relayName}@".serviceConfig.StandardInput == "socket"
        && config.systemd.services."${relayName}@".serviceConfig.StandardOutput == "socket";
    }
  ) admissionAgents;
in
{
  imports = [ researchFlake.nixosModules.default ];
  options.tentaflake.research = {
    admissionResearch = lib.mkOption {
      type = lib.types.attrsOf lib.types.raw;
      readOnly = true;
      internal = true;
      description = "Research-owned non-secret final process and relay expectations for explicitly requested Generic admission; not live readiness evidence.";
    };
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
  config = lib.mkMerge [
    {
      tentaflake.research.admissionResearch = lib.mapAttrs (
        _: projected: projected.spec
      ) admissionProjections;
    }
    (lib.mkIf (cfg.agents != { }) {
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
      ++ lib.mapAttrsToList (name: projected: {
        assertion = projected.valid;
        message = "Research admission for ${name} requires canonical final process, identity, namespace, config bindings and relay socket settings.";
      }) admissionProjections
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
            && (
              if instance.adapter == "generic" then
                lib.elem "research" instance.requestedCapabilities
              else
                adapters.${instance.adapter}.research.supported
            )
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
      ) cfg.agents;
      # gVisor defaults to denying host UDS. Permit opening projected sockets;
      # creation remains disabled, and mount policy admits only each exact relay.
      virtualisation.docker.daemon.settings.runtimes.runsc.path = lib.mkForce (lib.getExe runsc);
      virtualisation.podman.extraRuntimes = lib.mkForce [ runsc ];
    })
  ];
}
