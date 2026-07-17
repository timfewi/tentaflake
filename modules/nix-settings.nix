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
    # Daemon hardening: agents run in containers and never talk to the host
    # daemon, so only root and wheel need access.
    allowed-users = [
      "root"
      "@wheel"
    ];
    # sandbox = true is the Linux default; sandbox-fallback = false is not —
    # it closes the silent downgrade to unsandboxed builds.
    sandbox = true;
    sandbox-fallback = false;
    # Keep builds from filling the disk: GC down to 8 GiB free when less than
    # 2 GiB remains (complements the weekly nix.gc below).
    min-free = 2 * 1024 * 1024 * 1024;
    max-free = 8 * 1024 * 1024 * 1024;
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
