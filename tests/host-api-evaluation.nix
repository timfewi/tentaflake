{ pkgs }:
let
  builders = import ../lib {
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
  workerHost =
    decisions:
    evaluate {
      imports = [
        (builders.mkHermesAgent {
          name = "operator";
          autoStart = false;
        })
      ];
      tentaflake.hostApi = {
        enable = true;
        allowToolRequests = true;
        allowToolDecisions = decisions;
      };
      tentaflake.worker.agents.hermes-operator = {
        enable = true;
        workspace = "/var/lib/hermes-operator/workspace";
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
  == [ "-/var/lib/tentaflake-worker-hermes-operator" ];
assert
  controlledWorker.systemd.paths.tentaflake-worker-hermes-operator.pathConfig.PathExists
  == "/var/lib/tentaflake-worker-hermes-operator/.drain-request";
assert controlledWorker.systemd.services.tentaflake-api.serviceConfig.CapabilityBoundingSet == "";
assert pkgs.lib.hasInfix "tentaflake-host-api --config /nix/store/"
  controlledWorker.systemd.services.tentaflake-api.serviceConfig.ExecStart;
assert policies.hermes-operator.managed;
assert !policies.hermes-operator.research.enabled;
assert policies.hermes-operator.research.readiness == "unverified";
assert policies.hermes-operator.worker.enabled;
assert policies.hermes-operator.worker.limits.max_timeout_seconds == 900;
assert policies.hermes-operator.auto_start == false;
assert !(pkgs.lib.hasInfix "/var/lib" (builtins.toJSON policies));
assert !(pkgs.lib.hasInfix "Credential" (builtins.toJSON policies));
assert service.StateDirectoryMode == "0700";
assert service.RuntimeDirectoryMode == "0750";
assert service.UMask == "0077";
assert enabled.networking.firewall.allowedTCPPorts == disabled.networking.firewall.allowedTCPPorts;
pkgs.runCommand "tentaflake-host-api-evaluation" { } ''
  touch "$out"
''
