{ pkgs }:
let
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
  writable = evaluate {
    tentaflake.hostApi = {
      enable = true;
      allowActions = true;
      allowLogs = true;
    };
  };
  service = enabled.systemd.services.tentaflake-api.serviceConfig;
  failures =
    configuration:
    map (entry: entry.message) (pkgs.lib.filter (entry: !entry.assertion) configuration.assertions);
in
assert pkgs.lib.assertMsg (failures disabled == [ ]) (builtins.toJSON (failures disabled));
assert pkgs.lib.assertMsg (failures enabled == [ ]) (builtins.toJSON (failures enabled));
assert pkgs.lib.assertMsg (failures writable == [ ]) (builtins.toJSON (failures writable));
assert !(disabled.systemd.services ? tentaflake-api);
assert !(disabled.users.groups ? tentaflake-api);
assert !enabled.tentaflake.hostApi.allowActions;
assert !enabled.tentaflake.hostApi.allowLogs;
assert writable.tentaflake.hostApi.allowActions;
assert writable.tentaflake.hostApi.allowLogs;
assert !(enabled.users.users ? tentahq-ui);
assert !(enabled.environment.etc ? "tentaflake/cli.conf");
assert service.User == "root";
assert pkgs.lib.hasInfix "tentaflake-host-api --config /nix/store/" service.ExecStart;
assert service.Group == "tentaflake-api";
assert service.RestrictAddressFamilies == [ "AF_UNIX" ];
assert service.CapabilityBoundingSet == "";
assert service.AmbientCapabilities == "";
assert service.NoNewPrivileges;
assert service.ProtectSystem == "strict";
assert service.StateDirectoryMode == "0700";
assert service.RuntimeDirectoryMode == "0750";
assert service.UMask == "0077";
assert enabled.networking.firewall.allowedTCPPorts == disabled.networking.firewall.allowedTCPPorts;
pkgs.runCommand "tentaflake-host-api-evaluation" { } ''
  touch "$out"
''
