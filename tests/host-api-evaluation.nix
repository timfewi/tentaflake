{ pkgs }:
let
  mkAgent = import ../lib/mkAgent.nix {
    inherit pkgs;
    inherit (pkgs) lib;
  };
  evaluate =
    extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      specialArgs.researchFlake = null;
      modules = [
        ../modules/default.nix
        {
          system.stateVersion = "26.05";
          fileSystems."/" = {
            device = "/dev/disk/by-label/nixos";
            fsType = "btrfs";
          };
          boot.loader.grub.devices = [ "nodev" ];
          tentaflake = {
            adminUser = "operator";
            boot.enable = false;
            locale.enable = false;
            networking.enable = false;
            nixSettings.enable = false;
            packages.enable = false;
            shell.enable = false;
          };
        }
        extra
      ];
    }).config;
  disabled = evaluate { };
  enabled = evaluate { tentaflake.hostApi.enable = true; };
  noWorkerDependency = evaluate {
    tentaflake.hostApi.enable = true;
    tentaflake.worker.package = throw "disabled worker inspection must not force its package";
  };
  writable = evaluate {
    tentaflake.hostApi = {
      enable = true;
      allowActions = true;
      allowLogs = true;
    };
  };
  scoped = evaluate {
    imports = [
      (mkAgent {
        adapter = "hermes";
        name = "scoped";
        autoStart = false;
      })
    ];
    tentaflake.hostApi = {
      enable = true;
      peers = [
        {
          uid = 12345;
          agents = [ "agent-scoped" ];
          permissions = [
            "read"
            "run_read"
            "run_submit"
            "run_stop"
          ];
        }
      ];
    };
  };
  fleetNames = pkgs.lib.genList (index: "fleet-${toString index}") 300;
  fleetAgents = map (name: "agent-${name}") fleetNames;
  fleetPeer = {
    uid = 12345;
    agents = fleetAgents;
    permissions = [ "read" ];
  };
  fleetPeers = [
    fleetPeer
  ]
  ++ pkgs.lib.genList (index: {
    uid = 12346 + index;
    agents = [ (builtins.elemAt fleetAgents index) ];
    permissions = [ "read" ];
  }) 299;
  fleet = evaluate {
    # Synthetic stopped declarations isolate inventory/grant scale from admission.
    tentaflake.agentDeclarations = map (name: {
      adapter = "fixture";
      inherit name;
      container = "agent-${name}";
      unit = "docker-agent-${name}.service";
      stateDir = "/var/lib/tentaflake/agents/${name}";
      stateStorage = "/var/lib/tentaflake/agents/${name}/state";
      workspace = "/var/lib/tentaflake/agents/${name}/workspace";
      uid = 2000;
      gid = 2000;
      runnable = false;
    }) fleetNames;
    tentaflake.hostApi = {
      enable = true;
      peers = fleetPeers;
    };
  };
  oversizedPeerGrants = evaluate {
    tentaflake.agentDeclarations = fleet.tentaflake.agentDeclarations;
    tentaflake.hostApi = {
      enable = true;
      peers = pkgs.lib.genList (index: {
        uid = 12345 + index;
        agents = fleetAgents;
        permissions = [ "read" ];
      }) 300;
    };
  };
  unknownPeerAgent = evaluate {
    tentaflake.hostApi = {
      enable = true;
      peers = [
        {
          uid = 12345;
          agents = [ "agent-absent" ];
          permissions = [ "read" ];
        }
      ];
    };
  };
  workerHost =
    decisions:
    evaluate {
      imports = [
        (mkAgent {
          adapter = "hermes";
          name = "operator";
          autoStart = false;
        })
      ];
      tentaflake.hostApi = {
        enable = true;
        allowToolRequests = true;
        allowToolDecisions = decisions;
      };
      tentaflake.worker.agents.agent-operator = {
        enable = true;
        workspace = "/var/lib/tentaflake/agents/operator/workspace";
      };
    };
  inspectedWorker = workerHost false;
  controlledWorker = workerHost true;
  invalidDecisions = evaluate {
    tentaflake.hostApi = {
      enable = true;
      allowToolDecisions = true;
    };
  };
  policies = import ../lib/hostApiPolicy.nix {
    config = inspectedWorker;
    inherit (pkgs) lib;
  };
  piDurableTransport = import ../lib/piDurableTransport.nix {
    inherit pkgs;
    inherit (pkgs) lib;
  };
  # Synthetic declarations isolate host projection admission, not runtime acceptance.
  runnerHost =
    adapter: runnable: enable: selector: transportEnabled: extraOptions:
    evaluate {
      tentaflake = {
        hostApi.enable = true;
        piDurableRuntime = {
          inherit enable;
          transport = if transportEnabled then piDurableTransport.version else null;
          package = pkgs.hello;
          image = "localhost/fixture:reviewed";
          bundle = pkgs.runCommand "synthetic-pi-durable-bundle" {
            passthru = {
              imageVerificationVersion = 1;
              imageSelector = selector;
              runtimePackage = pkgs.hello;
            };
          } "mkdir $out";
        };
        agentDeclarations = [
          {
            inherit adapter runnable;
            name = "runner";
            container = "agent-runner";
            unit = "docker-agent-runner.service";
            stateDir = "/var/lib/tentaflake/agents/runner";
            stateStorage = "/var/lib/tentaflake/agents/runner/state";
            workspace = "/var/lib/tentaflake/agents/runner/workspace";
            uid = 10100;
            gid = 10100;
          }
        ];
      };
      virtualisation = {
        oci-containers = {
          backend = "docker";
          containers.agent-runner = {
            image = "localhost/fixture:reviewed";
            inherit extraOptions;
          };
        };
        docker.daemon.settings.runtimes.runsc-pi-durable.path = piDurableTransport.path;
      };
    };
  admittedRunner = runnerHost "pi-durable" true true "localhost/fixture:reviewed" true [ ];
  stoppedRunner = runnerHost "pi-durable" false true "localhost/fixture:reviewed" true [ ];
  disabledRunner = runnerHost "pi-durable" true false "localhost/fixture:reviewed" true [ ];
  otherRunner = runnerHost "hermes" true true "localhost/fixture:reviewed" true [ ];
  unreviewedRunner = runnerHost "pi-durable" true true "localhost/fixture:other" true [ ];
  unsupportedRunner = runnerHost "pi-durable" true true "localhost/fixture:reviewed" false [ ];
  remappedRunner = runnerHost "pi-durable" true true "localhost/fixture:reviewed" true [
    "--userns=private"
  ];
  generatedConfig =
    configuration:
    pkgs.lib.last (
      pkgs.lib.splitString " " configuration.systemd.services.tentaflake-api.serviceConfig.ExecStart
    );
  service = enabled.systemd.services.tentaflake-api.serviceConfig;
  failures =
    configuration:
    map (entry: entry.message) (pkgs.lib.filter (entry: !entry.assertion) configuration.assertions);
in
assert pkgs.lib.assertMsg (failures disabled == [ ]) (builtins.toJSON (failures disabled));
assert pkgs.lib.assertMsg (failures enabled == [ ]) (builtins.toJSON (failures enabled));
assert pkgs.lib.assertMsg (failures writable == [ ]) (builtins.toJSON (failures writable));
assert pkgs.lib.assertMsg (failures inspectedWorker == [ ]) (
  builtins.toJSON (failures inspectedWorker)
);
assert pkgs.lib.assertMsg (failures controlledWorker == [ ]) (
  builtins.toJSON (failures controlledWorker)
);
assert pkgs.lib.any (pkgs.lib.hasInfix "allowToolDecisions requires allowToolRequests") (
  failures invalidDecisions
);
assert failures scoped == [ ];
assert pkgs.lib.assertMsg (failures fleet == [ ]) (builtins.toJSON (failures fleet));
assert builtins.length (builtins.attrNames fleet.tentaflake.agentInstances) == 300;
assert fleet.tentaflake.hostApi.peers == fleetPeers;
assert builtins.length fleet.tentaflake.hostApi.peers == 300;
assert pkgs.lib.any (pkgs.lib.hasInfix "512 KiB") (failures oversizedPeerGrants);
assert scoped.systemd.services.tentaflake-api.serviceConfig.RuntimeDirectoryMode == "0711";
assert enabled.tentaflake.hostApi.peers == null;
assert scoped.tentaflake.hostApi.peers != null;
assert
  admittedRunner.systemd.services.tentaflake-api.serviceConfig.SupplementaryGroups == [ "10100" ];
assert admittedRunner.systemd.services.tentaflake-api.serviceConfig.CapabilityBoundingSet == "";
assert pkgs.lib.all
  (
    configuration:
    configuration.systemd.services.tentaflake-api.serviceConfig.SupplementaryGroups == [ ]
  )
  [
    enabled
    stoppedRunner
    disabledRunner
    otherRunner
    unreviewedRunner
    unsupportedRunner
    remappedRunner
  ];
assert pkgs.lib.any (pkgs.lib.hasInfix "exact declared agent IDs") (failures unknownPeerAgent);
assert !(disabled.systemd.services ? tentaflake-api);
assert !(disabled.users.groups ? tentaflake-api);
assert !enabled.tentaflake.hostApi.allowActions;
assert !enabled.tentaflake.hostApi.allowLogs;
assert !enabled.tentaflake.hostApi.allowToolRequests;
assert !enabled.tentaflake.hostApi.allowToolDecisions;
assert writable.tentaflake.hostApi.allowActions;
assert writable.tentaflake.hostApi.allowLogs;
assert !(enabled.users.users ? tentahq-ui);
assert !(enabled.environment.etc ? "tentaflake/cli.conf");
assert service.User == "root";
assert pkgs.lib.hasInfix "tentaflake-host-api --config /nix/store/" service.ExecStart;
assert pkgs.lib.hasInfix "tentaflake-host-api --config /nix/store/"
  noWorkerDependency.systemd.services.tentaflake-api.serviceConfig.ExecStart;
assert service.Group == "tentaflake-api";
assert service.RestrictAddressFamilies == [ "AF_UNIX" ];
assert service.CapabilityBoundingSet == "";
assert service.AmbientCapabilities == "";
assert service.NoNewPrivileges;
assert service.ProtectSystem == "strict";
assert service.ReadWritePaths == [ ];
assert inspectedWorker.systemd.services.tentaflake-api.serviceConfig.ReadWritePaths == [ ];
assert
  controlledWorker.systemd.services.tentaflake-api.serviceConfig.ReadWritePaths
  == [ "-/var/lib/tentaflake-worker-agent-operator" ];
assert
  controlledWorker.systemd.paths.tentaflake-worker-agent-operator.pathConfig.PathExists
  == "/var/lib/tentaflake-worker-agent-operator/.drain-request";
assert controlledWorker.systemd.services.tentaflake-api.serviceConfig.CapabilityBoundingSet == "";
assert pkgs.lib.hasInfix "tentaflake-host-api --config /nix/store/"
  controlledWorker.systemd.services.tentaflake-api.serviceConfig.ExecStart;
assert policies.agent-operator.managed;
assert !policies.agent-operator.research.enabled;
assert policies.agent-operator.research.readiness == "unverified";
assert policies.agent-operator.runtime == null;
assert policies.agent-operator.worker.enabled;
assert policies.agent-operator.worker.limits.max_timeout_seconds == 3600;
assert policies.agent-operator.auto_start == false;
assert !(pkgs.lib.hasInfix "/var/lib" (builtins.toJSON policies));
assert !(pkgs.lib.hasInfix "Credential" (builtins.toJSON policies));
assert service.StateDirectoryMode == "0700";
assert service.RuntimeDirectoryMode == "0750";
assert service.UMask == "0077";
assert enabled.networking.firewall.allowedTCPPorts == disabled.networking.firewall.allowedTCPPorts;
pkgs.runCommand "tentaflake-host-api-evaluation" { } ''
  ${pkgs.python3}/bin/python3 - ${generatedConfig admittedRunner} ${generatedConfig stoppedRunner} ${generatedConfig disabledRunner} ${generatedConfig otherRunner} ${generatedConfig unreviewedRunner} ${generatedConfig unsupportedRunner} ${generatedConfig remappedRunner} <<'PY'
  import json
  import sys
  with open(sys.argv[1]) as source:
      assert json.load(source)["runners"] == {
          "agent-runner": {
              "socket": "/run/tentaflake-pi-durable/agent-runner/control/runner.sock",
              "token": "/run/tentaflake-pi-durable/agent-runner/api-token",
              "transport": "runsc-pi-durable-v1",
          }
      }
  for path in sys.argv[2:]:
      with open(path) as source:
          assert json.load(source)["runners"] == {}
  PY
  ${pkgs.python3}/bin/python3 - ${generatedConfig fleet} <<'PY'
  import json
  import sys
  from pathlib import Path

  with open(sys.argv[1]) as source:
      configuration = json.load(source)
  expected = {f"agent-fleet-{number}" for number in range(300)}
  assert configuration["peer_grants"] == [{
      "uid": 12345,
      "agents": [f"agent-fleet-{number}" for number in range(300)],
      "permissions": ["read"],
  }] + [{
      "uid": 12346 + number,
      "agents": [f"agent-fleet-{number}"],
      "permissions": ["read"],
  } for number in range(299)]
  assert len(json.dumps(configuration["peer_grants"], separators=(",", ":")).encode()) <= 512 * 1024
  inventory = Path(configuration["inventory"]).read_bytes()
  assert len(inventory) <= 256 * 1024
  rows = inventory.decode().splitlines()
  assert len(rows) == 300
  assert {row.split("\t")[2] for row in rows} == expected
  assert set(configuration["policies"]) == expected
  PY
  touch "$out"
''
