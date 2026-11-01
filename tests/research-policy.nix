{ self, pkgs }:
let
  inherit (pkgs) lib;
  mkAgent = import ../lib/mkAgent.nix { inherit pkgs lib; };
  host = {
    system.stateVersion = "26.05";
    fileSystems."/" = {
      device = "/dev/disk/by-label/nixos";
      fsType = "btrfs";
    };
    boot.loader.grub.devices = [ "nodev" ];
    virtualisation.oci-containers.backend = "docker";
    tentaflake = {
      hostName = "research-policy-fixture";
      adminUser = "operator";
      security.profile = "balanced";
      boot.enable = false;
      locale.enable = false;
      networking.enable = false;
      nixSettings.enable = false;
      packages.enable = false;
      shell.enable = false;
      research.agents.agent-policy.uid = 62101;
    };
    services.secureResearch = {
      serviceUid = 4201;
      egressUid = 4202;
      vpnInterface = "fixture-vpn";
      resolvers = [ "9.9.9.9" ];
    };
  };
  evaluate =
    args: extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        self.nixosModules.default
        host
        (mkAgent (
          {
            adapter = "hermes";
            name = "policy";
            autoStart = false;
          }
          // args
        ))
        extra
      ];
    }).config;
  failures =
    config: map (entry: entry.message) (lib.filter (entry: !entry.assertion) config.assertions);
  denies =
    fragment: config:
    lib.any (entry: lib.hasInfix fragment entry.message && !entry.assertion) config.assertions;
  valid = evaluate { settings.agent.disabled_toolsets = [ "terminal" ]; } { };
  remote = evaluate { settings.mcp_servers.remote.url = "https://connector.example.org/mcp"; } { };
  missingMounts = evaluate { extraContainerConfig.volumes = [ ]; } { };
  legacy = evaluate { } {
    tentaflake.networking.enable = lib.mkForce true;
    tentaflake.broker.agents.agent-policy = {
      enable = true;
      networkName = "tf-policy";
      subnet = "10.203.44.0/30";
      gateway = "10.203.44.1";
      fetch = {
        enable = true;
        allowedHosts = [ "docs.example.org" ];
      };
    };
  };
  summary = evaluate { } {
    services.secureResearch.summarizeOrder = [ "openai" ];
  };
  duplicateUid = evaluate { } {
    services.secureResearch.containerClients.another.uid = 62101;
  };
  unregistered = evaluate { } {
    tentaflake.research.agents.hermes-unmanaged.uid = 62104;
    virtualisation.oci-containers.containers.hermes-unmanaged = {
      image = "registry.example.invalid/fixture@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
      autoStart = false;
    };
  };
  zeroValid = evaluate { } {
    imports = [
      (mkAgent {
        adapter = "zeroclaw";
        name = "zero-policy";
        autoStart = false;
      })
    ];
    tentaflake.research.agents.agent-zero-policy.uid = 62105;
  };
  zeroRemote = evaluate { } {
    imports = [
      (mkAgent {
        adapter = "zeroclaw";
        name = "zero-policy";
        autoStart = false;
        settings.mcp.servers = [
          {
            url = "https://connector.example.org/mcp";
            transport = "sse";
          }
        ];
      })
    ];
    tentaflake.research.agents.agent-zero-policy.uid = 62105;
  };
  secureExample = evaluate { } {
    imports = [
      (import ../examples/adapter-secure.nix {
        # Synthetic policy fixtures deliberately exercise the internal adapter API.
        systemFromData = { file }: {
          imports = (import ../lib/agentsFromData.nix { inherit pkgs lib; }) { inherit file; };
        };
        systemFile = ./fixtures/adapter-input/secure-example.toml;
        agentName = "assistant";
        model = "fixture-model";
        inputMicrousdPerMillion = 1;
        outputMicrousdPerMillion = 1;
        providerCredentialFile = "/run/credentials/fixture-provider";
        upstreamBaseUrl = "https://api.example.invalid/v1/";
        vpnInterface = "fixture-vpn";
        researchPolicy.resolvers = [ "9.9.9.9" ];
      })
    ];
    tentaflake.networking.enable = lib.mkForce true;
    tentaflake.research.agents.agent-policy.uid = lib.mkForce 62106;
  };
  client = import ../lib/researchClient.nix {
    inherit lib pkgs;
    config = valid;
    containerName = "agent-policy";
    settings.agent.disabled_toolsets = [ "terminal" ];
    adapter = "hermes";
  };
  record = lib.findFirst (line: lib.hasPrefix "agent\tagent-policy\t" line) "" (
    lib.splitString "\n" valid.environment.etc."tentaflake/security.tsv".text
  );
  declarativeFixture = ./fixtures/research-projection/tentaflake.toml;
  declarativeDesired = builtins.fromTOML (builtins.readFile declarativeFixture);
  evaluateDeclarative =
    extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        self.nixosModules.default
        (
          host
          // {
            tentaflake = builtins.removeAttrs host.tentaflake [ "research" ];
            services = { };
          }
        )
        (self.lib.${pkgs.stdenv.hostPlatform.system}.systemFromData { file = declarativeFixture; })
        extra
      ];
    }).config;
  declarative = evaluateDeclarative { inherit (host) services; };
  declarativeMissingPolicy = evaluateDeclarative { };
  declarativeDisabledService = evaluateDeclarative {
    services = lib.recursiveUpdate host.services {
      secureResearch.enable = lib.mkForce false;
    };
  };
  declarativeCollision = evaluateDeclarative {
    inherit (host) services;
    users.users.fixture-collision = {
      uid = 62111;
      isSystemUser = true;
      group = "fixture-collision";
    };
    users.groups.fixture-collision = { };
  };
  transport = import ../lib/researchTransport.nix { inherit lib pkgs; };
  security = import ../lib/containerSecurity.nix { inherit lib; };
  runtimePolicy =
    backend: grant: runtimeName:
    security.apply {
      inherit backend runtimeName;
      researchPolicyEnabled = grant;
      profile = "balanced";
      name = "agent-runtime-fixture";
      owner = "10000:10000";
      baseConfig.image = "registry.example.invalid/fixture@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
      resources = valid.tentaflake.security.resources;
      pidsLimit = 128;
      allowedWritableSources = [ ];
      allowedWritableDestinations = [ ];
    };
  runtimePolicyValid =
    backend: grant: name:
    lib.all (entry: entry.assertion) (runtimePolicy backend grant name).assertions;
  podmanDeclarative = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.backend = lib.mkForce "podman";
  };
  runtimeOverride = evaluateDeclarative {
    inherit (host) services;
    virtualisation.docker.daemon.settings.runtimes.runsc-research.path = lib.mkForce "/bin/runsc";
  };
  runtimeArguments = evaluateDeclarative {
    inherit (host) services;
    virtualisation.docker.daemon.settings.runtimes.runsc-research.runtimeArgs = [ "--host-uds=all" ];
  };
  runtimeGlobalDefault = evaluateDeclarative {
    inherit (host) services;
    virtualisation.docker.daemon.settings."default-runtime" = "runsc-research";
  };
  podmanRuntimeOverride = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.backend = lib.mkForce "podman";
    virtualisation.containers.containersConf.settings.engine.runtimes.runsc-research = lib.mkForce [
      "/bin/runsc"
    ];
  };
  podmanRuntimeGlobalDefault = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.backend = lib.mkForce "podman";
    virtualisation.containers.containersConf.settings.engine.runtime = "runsc-research";
  };
  ungrantedRuntime = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.containers.agent-offline.extraOptions = lib.mkForce [
      "--runtime=runsc-research"
    ];
  };
  runtimeRecord =
    configuration: id:
    let
      line = lib.findFirst (lib.hasPrefix "agent\t${id}\t") "" (
        lib.splitString "\n" configuration.environment.etc."tentaflake/security.tsv".text
      );
    in
    lib.elemAt (lib.splitString "\t" (builtins.unsafeDiscardStringContext line)) 30;
  runtimes = import ../lib/gvisorRuntimes.nix { inherit lib pkgs; };
  registrationAccepts =
    configuration: change: runtimes.configurationSupported (lib.recursiveUpdate configuration change);
  stockOverride = evaluateDeclarative {
    inherit (host) services;
    virtualisation.docker.daemon.settings.runtimes.runsc.path = lib.mkForce transport.path;
  };
  podmanStockOverride = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.backend = lib.mkForce "podman";
    virtualisation.containers.containersConf.settings.engine.runtimes.runsc = lib.mkForce [
      transport.path
    ];
  };
  ungrantedPi = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.containers.agent-offline.extraOptions = lib.mkForce [
      "--runtime=runsc-pi"
    ];
  };
  podmanUngrantedPi = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.backend = lib.mkForce "podman";
    virtualisation.oci-containers.containers.agent-offline.preRunExtraOptions = lib.mkForce [
      "--runtime"
      "runsc-pi"
    ];
  };
  ungrantedDirectPath = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.containers.agent-offline.extraOptions = lib.mkForce [
      "--runtime=${transport.path}"
    ];
  };
  unrelatedConsumer =
    backend:
    evaluateDeclarative {
      inherit (host) services;
      virtualisation = {
        oci-containers.backend = lib.mkForce backend;
        oci-containers.containers.consumer = {
          image = "registry.example.invalid/fixture@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
          autoStart = false;
          extraOptions = lib.optional (backend == "docker") "--runtime=consumer";
          preRunExtraOptions = lib.optionals (backend == "podman") [
            "--runtime"
            "consumer"
          ];
        };
        docker.daemon.settings = lib.mkIf (backend == "docker") {
          runtimes.consumer.path = "/nix/store/synthetic-consumer/bin/consumer";
          "default-runtime" = "consumer";
        };
        containers.containersConf.settings.engine = lib.mkIf (backend == "podman") {
          runtimes.consumer = [ "/nix/store/synthetic-consumer/bin/consumer" ];
          runtime = "consumer";
        };
      };
    };
  unmanagedElevated = evaluateDeclarative {
    inherit (host) services;
    virtualisation.oci-containers.containers.consumer = {
      image = "registry.example.invalid/fixture@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
      autoStart = false;
      extraOptions = [ "--runtime=${runtimes.pi.path}" ];
    };
  };
in
# Negative fixtures must inspect their own denial without forcing unrelated
# assertions (NixOS traces duplicate IDs when evaluating its uniqueness check).
assert denies "fixture denial" {
  assertions = [
    {
      message = "unrelated assertion";
      assertion = throw "denies evaluated an unrelated assertion";
    }
    {
      message = "fixture denial";
      assertion = true;
    }
    {
      message = "fixture denial";
      assertion = false;
    }
  ];
};
assert
  !(denies "fixture denial" {
    assertions = [
      {
        message = "fixture denial";
        assertion = true;
      }
      {
        message = "unrelated assertion";
        assertion = throw "denies evaluated an unrelated assertion";
      }
    ];
  });
assert lib.elemAt (lib.splitString "\t" record) 10 == "true";
assert failures valid == [ ];
assert import ./research-vpn-observer.nix { inherit self pkgs; };
assert valid.systemd.services.agent-research-egress-control.serviceConfig.PrivateNetwork;
assert valid.systemd.services.agent-research-egress-control.bindsTo == [ "nftables.service" ];
assert lib.all (name: lib.elem name client.settings.agent.disabled_toolsets) [
  "terminal"
  "web"
  "browser"
];
assert denies "only local stdio MCP" remote;
assert denies "retain its exact read-only" missingMounts;
assert denies "disable its legacy fetch broker" legacy;
assert denies "leave summarization disabled" summary;
assert denies "distinct upstream client UIDs" duplicateUid;
assert denies "declared registered adapter" unregistered;
assert failures zeroValid == [ ];
assert denies "only local stdio MCP" zeroRemote;
assert lib.assertMsg (failures secureExample == [ ]) (builtins.toJSON (failures secureExample));
assert secureExample.virtualisation.oci-containers.containers.agent-assistant.autoStart;
assert failures declarative == [ ];
assert declarative.services.secureResearch.enable;
assert declarative.tentaflake.research.agents == declarativeDesired.research.agents;
assert declarative.services.secureResearch.containerClients == declarativeDesired.research.agents;
assert declarative.services.secureResearch.vpnInterface == "fixture-vpn";
assert !declarative.services.secureResearch.vpnObserver.enable;
assert declarative.services.secureResearch.providers == { };
assert declarative.services.secureResearch.summarizeOrder == [ ];
assert lib.all (
  id:
  let
    container = declarative.virtualisation.oci-containers.containers.${id};
  in
  declarative.tentaflake.agentInstances.${id}.runnable
  && !container.autoStart
  && lib.elem "/run/tentaflake-research/${id}:/run/tentaflake-research:ro" container.volumes
  && builtins.hasAttr "tentaflake-research-${id}" declarative.systemd.sockets
) (builtins.attrNames declarativeDesired.research.agents);
assert denies "explicit distinct service/egress UIDs" declarativeMissingPolicy;
assert denies "explicit VPN interface and public resolvers" declarativeMissingPolicy;
assert denies "secure profile and the isolated Research service" declarativeDisabledService;
assert denies "must not be shared with other host users" declarativeCollision;
assert failures podmanDeclarative == [ ];
assert transport.supported declarative;
assert transport.supported podmanDeclarative;
assert !transport.supported { };
assert
  declarative.virtualisation.docker.daemon.settings.runtimes.runsc == {
    path = lib.getExe' pkgs.gvisor "runsc";
  };
assert lib.elem pkgs.gvisor podmanDeclarative.virtualisation.podman.extraRuntimes;
assert
  declarative.virtualisation.docker.daemon.settings.runtimes.runsc-research == {
    inherit (transport) path;
  };
assert
  podmanDeclarative.virtualisation.containers.containersConf.settings.engine.runtimes.runsc-research
  == [ transport.path ];
assert lib.all
  (
    configuration:
    let
      backend = configuration.virtualisation.oci-containers.backend;
      containers = configuration.virtualisation.oci-containers.containers;
    in
    transport.selected backend containers.agent-offline "runsc"
    && lib.all (
      id:
      transport.selected backend containers.${id} transport.name
      &&
        builtins.fromJSON (runtimeRecord configuration id) == {
          inherit (transport) name path;
        }
    ) (builtins.attrNames declarativeDesired.research.agents)
    &&
      builtins.fromJSON (runtimeRecord configuration "agent-offline") == {
        name = "runsc";
        path = lib.getExe' pkgs.gvisor "runsc";
      }
  )
  [
    declarative
    podmanDeclarative
  ];
assert lib.all (backend: runtimePolicyValid backend true "runsc-research") [
  "docker"
  "podman"
];
assert runtimePolicyValid "docker" false "runsc";
assert !(runtimePolicyValid "docker" false "runsc-research");
assert !(runtimePolicyValid "docker" true "runsc");
assert !(runtimePolicyValid "docker" true "arbitrary");
assert !(runtimePolicyValid "docker" true "runsc-pi");
assert denies "exact per-agent runtime registration" runtimeOverride;
assert denies "exact per-agent runtime registration" runtimeArguments;
assert denies "exact per-agent runtime registration" runtimeGlobalDefault;
assert denies "exact per-agent runtime registration" podmanRuntimeOverride;
assert denies "exact per-agent runtime registration" podmanRuntimeGlobalDefault;
assert denies "exact admitted runtime" ungrantedRuntime;
assert runtimeRecord runtimeOverride "agent-researcher" == "-";
assert runtimeRecord ungrantedRuntime "agent-offline" == "-";
assert denies "exact backend-specific stock runsc registration" stockOverride;
assert denies "exact backend-specific stock runsc registration" podmanStockOverride;
assert denies "exact admitted runtime" ungrantedPi;
assert denies "exact admitted runtime" podmanUngrantedPi;
assert denies "exact admitted runtime" ungrantedDirectPath;
assert lib.all (backend: failures (unrelatedConsumer backend) == [ ]) [
  "docker"
  "podman"
];
assert denies "exact admitted runtime" unmanagedElevated;
assert
  !(registrationAccepts declarative {
    virtualisation.docker.daemon.settings.runtimes.runsc.runtimeArgs = [ "--host-uds=all" ];
  });
assert
  !(registrationAccepts podmanDeclarative {
    virtualisation.containers.containersConf.settings.engine.runtimes.runsc = [
      runtimes.stock.path
      "--host-uds=all"
    ];
  });
assert
  !(registrationAccepts declarative {
    virtualisation.containers.containersConf.settings.engine.runtimes.inactive-alias = [
      transport.path
    ];
  });
assert
  !(registrationAccepts podmanDeclarative {
    virtualisation.docker.daemon.settings.runtimes.inactive-alias.path = runtimes.pi.path;
  });
assert
  !(registrationAccepts declarative {
    virtualisation.docker.daemon.settings.runtimes.alias.path = transport.path;
  });
assert
  !(registrationAccepts declarative {
    virtualisation.docker.daemon.settings.runtimes.alias.path =
      "${transport.runtime}/bin/../bin/runsc-research";
  });
assert
  !(registrationAccepts declarative {
    virtualisation.docker.daemon.settings.runtimes.alias = {
      inherit (runtimes.stock) path;
      runtimeArgs = [ "--host-uds=open" ];
    };
  });
assert
  !(registrationAccepts declarative {
    virtualisation.docker.daemon.settings = {
      runtimes.alias.path = transport.path;
      "default-runtime" = "alias";
    };
  });
assert
  !(registrationAccepts declarative {
    virtualisation.docker.daemon.settings."default-runtime" = transport.path;
  });
assert
  !(registrationAccepts podmanDeclarative {
    virtualisation.containers.containersConf.settings.engine.runtimes.alias = [ transport.path ];
  });
assert
  !(registrationAccepts podmanDeclarative {
    virtualisation.containers.containersConf.settings.engine = {
      runtimes.alias = [ runtimes.pi.path ];
      runtime = "alias";
    };
  });
assert runtimes.selectorElevated declarative transport.path;
assert runtimes.selectorElevated declarative runtimes.pi.path;
assert !runtimes.selectorElevated declarative "consumer";
assert
  runtimes.selectors {
    extraOptions = [ "--runtime=${transport.path}" ];
    preRunExtraOptions = [
      "--runtime"
      "runsc-pi"
    ];
  } == [
    transport.path
    "runsc-pi"
  ];
pkgs.runCommand "tentaflake-research-policy" { } "touch $out"
