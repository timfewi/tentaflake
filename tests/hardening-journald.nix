{ pkgs }:
let
  inherit (pkgs) lib;
  eval =
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
          virtualisation.oci-containers.backend = "docker";
          tentaflake = {
            adminUser = "operator";
            security.profile = "balanced";
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
  enabled = eval { };
  hostOverride = eval {
    services.journald.settings.Journal = {
      SystemMaxUse = "256M";
      RuntimeMaxUse = "64M";
    };
  };
  disabled = eval {
    tentaflake.security.profile = lib.mkForce "dev";
    tentaflake.hardening.enable = lib.mkForce false;
  };
  legacy = eval { services.journald.extraConfig = "SystemMaxUse=500M"; };
  failed = config: map (item: item.message) (lib.filter (item: !item.assertion) config.assertions);
  selected =
    config:
    lib.getAttrs [
      "RateLimitIntervalSec"
      "RateLimitBurst"
      "Compress"
      "SystemMaxUse"
    ] config.services.journald.settings.Journal;
  expected = {
    RateLimitIntervalSec = "5s";
    RateLimitBurst = 1000;
    Compress = "yes";
    SystemMaxUse = "500M";
  };
  journal = enabled.environment.etc."systemd/journald.conf".text;
  hostJournal = hostOverride.environment.etc."systemd/journald.conf".text;
in
assert failed enabled == [ ];
assert failed hostOverride == [ ];
assert failed disabled == [ ];
assert selected enabled == expected;
assert selected hostOverride == expected // { SystemMaxUse = "256M"; };
assert hostOverride.services.journald.settings.Journal.RuntimeMaxUse == "64M";
assert lib.all (line: lib.count (item: item == line) (lib.splitString "\n" journal) == 1) [
  "RateLimitIntervalSec=5s"
  "RateLimitBurst=1000"
  "Compress=yes"
  "SystemMaxUse=500M"
];
assert lib.hasInfix "SystemMaxUse=256M\n" hostJournal;
assert !(lib.hasInfix "SystemMaxUse=500M\n" hostJournal);
assert lib.hasInfix "RuntimeMaxUse=64M\n" hostJournal;
assert lib.all (name: !(builtins.hasAttr name disabled.services.journald.settings.Journal)) [
  "RateLimitIntervalSec"
  "RateLimitBurst"
  "Compress"
  "SystemMaxUse"
];
assert lib.any (
  message:
  lib.hasInfix "services.journald.extraConfig" message
  && lib.hasInfix "services.journald.settings.Journal" message
) (failed legacy);
true
