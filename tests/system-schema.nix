{ pkgs, hostConfiguration }:
let
  inherit (pkgs) lib;
  validate = import ../lib/systemSchema.nix { inherit lib; };
  builders = import ../lib { inherit pkgs lib; };
  corpus = builtins.fromJSON (builtins.readFile ../schema/system-v1-fixtures.json);
  accepts = data: (builtins.tryEval (builtins.deepSeq (validate data) true)).success;
  repeat = count: lib.concatStrings (builtins.genList (_: "a") count);
  agentName = count: {
    schemaVersion = 1;
    agents = [
      {
        adapter = "hermes";
        name = repeat count;
        autoStart = false;
      }
    ];
  };
  hostName = name: {
    schemaVersion = 1;
    agents = [ ];
    system.hostName = name;
  };
  rejectsModule =
    args: !(builtins.tryEval (builtins.deepSeq (builders.systemFromData args) true)).success;
  fixture = ./fixtures/system-projection/tentaflake.toml;
  desired = builtins.fromTOML (builtins.readFile fixture);
  descriptor = builtins.fromJSON (builtins.readFile ../schema/system-v1.json);
  projected =
    (lib.evalModules {
      modules = [
        { _module.freeformType = lib.types.attrsOf lib.types.anything; }
        (builders.systemFromData { file = fixture; })
      ];
    }).config;
  # Compose the real built-in host and NixOS modules, replacing only its input
  # file. This catches bootstrap priority conflicts (including option aliases)
  # that the freeform projection check cannot detect.
  composed =
    (hostConfiguration.extendModules {
      specialArgs.systemFromData = _: builders.systemFromData { file = fixture; };
    }).config;
  renamedWithShell =
    (hostConfiguration.extendModules {
      specialArgs.systemFromData = _: builders.systemFromData { file = fixture; };
      modules = [ { tentaflake.shell.enable = lib.mkForce true; } ];
    }).config;
  projectionsMatch = lib.all (
    section:
    lib.all (
      field:
      let
        spec = descriptor.sections.${section}.${field};
      in
      if spec ? path then lib.attrByPath spec.path null projected == desired.${section}.${field} else true
    ) (builtins.attrNames desired.${section})
  ) (builtins.attrNames descriptor.sections);
  serviceFleet = count: {
    schemaVersion = 1;
    agents = builtins.genList (i: {
      name = "a${toString i}";
      adapter = "hermes";
      autoStart = false;
    }) count;
    hostApi = {
      enable = true;
      peers = builtins.genList (i: {
        uid = i + 1;
        agents = [ "agent-a${toString i}" ];
        permissions = [ "read" ];
      }) count;
    };
    workloadMapping = {
      enable = true;
      bindings = builtins.listToAttrs (
        builtins.genList (i: {
          name = "agent-a${toString i}";
          value = {
            tenant_id = "tenant";
            workload_id = "w${toString i}";
            policy_revision = "r";
          };
        }) count
      );
    };
    storage = {
      enable = true;
      paths = builtins.genList (i: "/var/lib/tentaflake/agents/a${toString i}") count;
      repositoryFile = "/run/backup/repository";
      passwordFile = "/run/backup/password";
    };
  };
  largeServiceFleet =
    let
      fleet = serviceFleet 300;
    in
    fleet
    // {
      hostApi = fleet.hostApi // {
        peers = fleet.hostApi.peers ++ [
          {
            uid = 301;
            agents = map (agent: "agent-${agent.name}") fleet.agents;
            permissions = [ "read" ];
          }
        ];
      };
      storage = fleet.storage // {
        paths = lib.concatMap (agent: [
          "/var/lib/tentaflake/agents/${agent.name}"
          "/var/lib/tentaflake/agents/${agent.name}/workspace"
          "/var/lib/tentaflake-broker-llm-agent-${agent.name}"
        ]) fleet.agents;
      };
    };
  runnerFleet = {
    schemaVersion = 1;
    agents = builtins.genList (i: {
      name = "runner-${toString i}";
      adapter = "pi-durable";
      autoStart = false;
    }) 300;
    hostApi = {
      enable = true;
      peers = [
        {
          uid = 1000;
          agents = builtins.genList (i: "agent-runner-${toString i}") 300;
          permissions = [
            "run_read"
            "run_submit"
            "run_stop"
          ];
        }
      ];
    };
  };
  serviceFixture = ./fixtures/service-projection/tentaflake.toml;
  serviceDesired = builtins.fromTOML (builtins.readFile serviceFixture);
  serviceProjection =
    (lib.evalModules {
      modules = [
        { _module.freeformType = lib.types.attrsOf lib.types.anything; }
        { inherit (builders.systemFromData { file = serviceFixture; }) config; }
      ];
    }).config.tentaflake;
  researchFixture = ./fixtures/research-projection/tentaflake.toml;
  researchDesired = builtins.fromTOML (builtins.readFile researchFixture);
  researchProjection =
    (lib.evalModules {
      modules = [
        { _module.freeformType = lib.types.attrsOf lib.types.anything; }
        { inherit (builders.systemFromData { file = researchFixture; }) config; }
      ];
    }).config;
  disabledResearchProjection =
    (lib.evalModules {
      modules = [
        { _module.freeformType = lib.types.attrsOf lib.types.anything; }
        { inherit (builders.systemFromData { file = fixture; }) config; }
        {
          services.secureResearch.enable = true;
          tentaflake.research.agents.agent-existing.uid = 62113;
        }
      ];
    }).config;
  researchFleet = count: {
    schemaVersion = 1;
    agents = builtins.genList (i: {
      name = "r${toString i}";
      adapter = "hermes";
      autoStart = false;
    }) count;
    research = {
      enable = true;
      agents = builtins.listToAttrs (
        builtins.genList (i: {
          name = "agent-r${toString i}";
          value.uid = i + 1;
        }) count
      );
    };
  };
in
assert accepts (
  (serviceFleet 128)
  // {
    hostApi = {
      enable = false;
    };
    workloadMapping = {
      enable = false;
    };
  }
);
assert accepts (
  (serviceFleet 129)
  // {
    hostApi = {
      enable = false;
    };
    workloadMapping = {
      enable = false;
    };
  }
);
assert lib.all accepts corpus.valid;
assert lib.all (data: !accepts data) corpus.invalid;
assert accepts (agentName 57);
assert !accepts (agentName 58);
assert accepts (hostName (repeat 63));
assert !accepts (hostName (repeat 64));
assert !accepts (hostName "trailing-");
assert accepts (serviceFleet 64);
assert accepts (
  (serviceFleet 65)
  // {
    workloadMapping = {
      enable = false;
    };
  }
);
assert accepts (
  (serviceFleet 65)
  // {
    hostApi = {
      enable = false;
    };
  }
);
assert accepts (serviceFleet 300);
assert accepts largeServiceFleet;
assert
  !accepts (
    largeServiceFleet
    // {
      hostApi = largeServiceFleet.hostApi // {
        peers = largeServiceFleet.hostApi.peers ++ [ (builtins.head largeServiceFleet.hostApi.peers) ];
      };
    }
  );
assert
  !accepts (
    largeServiceFleet
    // {
      storage = largeServiceFleet.storage // {
        paths = largeServiceFleet.storage.paths ++ [ (builtins.head largeServiceFleet.storage.paths) ];
      };
    }
  );
assert
  !accepts (
    largeServiceFleet
    // {
      workloadMapping = largeServiceFleet.workloadMapping // {
        bindings = largeServiceFleet.workloadMapping.bindings // {
          agent-a299 = largeServiceFleet.workloadMapping.bindings.agent-a0;
        };
      };
    }
  );
assert accepts runnerFleet;
assert
  !accepts (
    runnerFleet
    // {
      agents = lib.imap0 (
        i: agent: if i == 299 then agent // { adapter = "hermes"; } else agent
      ) runnerFleet.agents;
    }
  );
assert
  !accepts {
    schemaVersion = 1;
    agents = [ ];
    connectors = {
      enable = true;
      configFile = "/run/${repeat 252}";
    };
  };
assert
  !accepts (
    (serviceFleet 1)
    // {
      workloadMapping = {
        enable = true;
        bindings.agent-a0 = {
          tenant_id = repeat 65;
          workload_id = "w";
          policy_revision = "r";
        };
      };
    }
  );
assert projectionsMatch;
assert lib.all (
  section:
  lib.all (
    field:
    let
      spec = descriptor.sections.${section}.${field};
    in
    if spec ? path then lib.attrByPath spec.path null composed == desired.${section}.${field} else true
  ) (builtins.attrNames desired.${section})
) (builtins.attrNames descriptor.sections);
assert composed.time.timeZone == "Europe/Berlin";
assert composed.networking.hostName == "schema-host";
assert
  composed.tentaflake.configurationName == hostConfiguration.config.tentaflake.configurationName;
assert composed.tentaflake.configurationName != composed.networking.hostName;
assert lib.hasInfix "host_name=schema-host"
  renamedWithShell.environment.etc."tentaflake/cli.conf".text;
assert lib.hasInfix "configuration_name=${hostConfiguration.config.tentaflake.configurationName}"
  renamedWithShell.environment.etc."tentaflake/cli.conf".text;
assert composed.system.stateVersion == "26.05";
assert composed.tentaflake.shell.enable == false;
assert !(composed.environment.etc ? "tentaflake/cli.conf");
assert composed.tentaflake.management.enable == false;
assert composed.tentaflake.tailscale.enable == false;
assert composed.services.tailscale.enable == false;
assert composed.tentaflake.hostApi.peers == desired.hostApi.peers;
assert composed.systemd.services.tentaflake-api.serviceConfig.User == "root";
assert composed.tentaflake.integrations.configFile == desired.connectors.configFile;
assert lib.hasInfix "serve-advisory"
  composed.systemd.services.tentaflake-integrations.serviceConfig.ExecStart;
assert composed.tentaflake.workloadMapping.enable;
assert composed.systemd.services.tentaflake-workloads.serviceConfig.User == "root";
assert serviceProjection.hostApi.peers == serviceDesired.hostApi.peers;
assert serviceProjection.workloadMapping.bindings == serviceDesired.workloadMapping.bindings;
assert serviceProjection.integrations.advisory.enable;
assert serviceProjection.integrations.configFile == serviceDesired.connectors.configFile;
assert accepts (researchFleet 128);
assert accepts (researchFleet 129);
assert accepts (researchFleet 300);
assert
  !accepts (
    let
      fleet = researchFleet 300;
    in
    fleet
    // {
      research = fleet.research // {
        agents = fleet.research.agents // {
          agent-r299.uid = 1;
        };
      };
    }
  );
assert researchProjection.services.secureResearch.enable;
assert researchProjection.tentaflake.research.agents == researchDesired.research.agents;
assert
  builtins.attrNames researchProjection == [
    "services"
    "tentaflake"
  ];
assert builtins.attrNames researchProjection.services.secureResearch == [ "enable" ];
assert disabledResearchProjection.services.secureResearch.enable;
assert
  disabledResearchProjection.tentaflake.research.agents == {
    agent-existing.uid = 62113;
  };
assert builtins.attrNames projected == [ "tentaflake" ];
assert builtins.attrNames (builtins.functionArgs builders.systemFromData) == [ "file" ];
assert !(builders ? agentsFromData);
assert rejectsModule { file = ./fixtures/adapter-input/legacy-only/agents.json; };
assert rejectsModule { file = ./fixtures/adapter-input/conflicting-inventories/agents.toml; };
assert rejectsModule {
  file = ./fixtures/adapter-input/conflicting-inventories + "/tentaflake.toml";
};
assert rejectsModule { file = ./fixtures + "/missing-system/tentaflake.toml"; };
pkgs.runCommand "tentaflake-system-schema" { } "touch $out"
