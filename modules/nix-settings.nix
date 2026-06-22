{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake;
in
lib.mkIf cfg.nixSettings.enable {
  nixpkgs.config.allowUnfree = cfg.allowUnfree;

  nix.settings = {
    experimental-features = [
      "nix-command"
      "flakes"
    ];
    trusted-users = [
      "root"
      cfg.adminUser
    ];
  };

  nix.gc = {
    automatic = true;
    dates = "weekly";
    options = "--delete-older-than 14d";
  };

  nix.optimise = {
    automatic = true;
    dates = [ "weekly" ];
    persistent = true;
  };
}
