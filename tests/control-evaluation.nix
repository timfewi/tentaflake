{ pkgs }:
let
  inherit (pkgs) lib;
  evaluate =
    extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        ../modules/control.nix
        {
          system.stateVersion = "26.05";
          fileSystems."/" = {
            device = "/dev/disk/by-label/synthetic";
            fsType = "ext4";
          };
          boot.loader.grub.devices = [ "nodev" ];
        }
        extra
      ];
    }).config;
  disabled = evaluate { };
  enabled = evaluate {
    tentaflake.control = {
      enable = true;
      uid = 31234;
      peers = [
        {
          uid = 31235;
          roles = [
            "read"
            "propose"
            "approve"
            "build"
          ];
        }
      ];
      hashes = {
        policy = lib.concatStrings (builtins.genList (_: "1") 64);
        compiler = lib.concatStrings (builtins.genList (_: "2") 64);
        inputs = lib.concatStrings (builtins.genList (_: "3") 64);
      };
    };
  };
  building = evaluate {
    tentaflake.control = {
      inherit (enabled.tentaflake.control)
        enable
        uid
        peers
        hashes
        ;
      build = {
        enable = true;
        daemonReady = true;
        template = "/nix/store/00000000000000000000000000000000-synthetic-template";
        templateHash = lib.concatStrings (builtins.genList (_: "4") 64);
        target = "synthetic";
      };
    };
  };
  buildSettings = builtins.fromJSON (
    builtins.unsafeDiscardStringContext (
      builtins.readFile building.environment.etc."tentaflake-control.json".source
    )
  );
  unit = enabled.systemd.services.tentaflake-control.serviceConfig;
  failures = cfg: map (entry: entry.message) (lib.filter (entry: !entry.assertion) cfg.assertions);
in
assert failures disabled == [ ];
assert failures enabled == [ ];
assert failures building == [ ];
assert !(disabled.systemd.services ? tentaflake-control);
assert !(disabled.users.users ? tentaflake-control);
assert enabled.users.users.tentaflake-control.uid == 31234;
assert unit.User == "tentaflake-control";
assert unit.NoNewPrivileges && unit.PrivateNetwork && unit.PrivateDevices;
assert unit.ProtectSystem == "strict";
assert unit.RestrictAddressFamilies == [ "AF_UNIX" ];
assert unit.CapabilityBoundingSet == "" && unit.AmbientCapabilities == "";
# Keep the live parent visible/read-only; never pin a desired file or either
# observation symlink to the inode that existed when the service started.
assert unit.ReadOnlyPaths == [ "/run" ];
assert !(lib.elem enabled.tentaflake.control.desiredSource unit.ReadOnlyPaths);
assert !(lib.elem "/run/current-system" unit.ReadOnlyPaths);
assert !(lib.elem "/run/booted-system" unit.ReadOnlyPaths);
assert
  unit.ReadWritePaths == [
    "/var/lib/tentaflake-control"
    "/run/tentaflake-control"
  ];
assert unit.InaccessiblePaths == [ "/nix/var/nix/daemon-socket" ];
assert unit.MemoryMax == "512M" && unit.TasksMax == 32;
assert lib.hasInfix "/bin/tentaflake-control serve /nix/store/" unit.ExecStart;
pkgs.runCommand "tentaflake-control-evaluation"
  {
    configuredNix = building.nix.package;
    resolvedNix = buildSettings.builder.nix;
  }
  ''
    test -e "$configuredNix/bin/nix"
    test -f "$resolvedNix"
    test ! -L "$resolvedNix"
    case "$resolvedNix" in /nix/store/*) ;; *) exit 1;; esac
    touch "$out"
  ''
