{
  config,
  lib,
  profile ? "installed",
  ...
}:
let
  cfg = config.tentaflake;
in
{
  imports = lib.optionals (profile == "installed") [
    ./hardware-configuration.nix
  ];

  # ── OCI container backend for agent environments ──
  virtualisation.oci-containers.backend = cfg.containerBackend;
  virtualisation.docker = lib.mkIf (profile == "installed" && cfg.containerBackend == "docker") {
    enable = true;
    autoPrune.enable = true;
  };

  # Docker group access is root-equivalent. Keep it only in the explicitly
  # compatibility-oriented dev profile; secure profiles never grant the daemon
  # socket to the login user.
  users.users.${cfg.adminUser}.extraGroups = lib.optional (
    profile == "installed" && cfg.containerBackend == "docker" && cfg.security.profile == "dev"
  ) "docker";

  tentaflake.profile = profile;
  system.stateVersion = cfg.stateVersion;
}
