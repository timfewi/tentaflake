{
  config,
  lib,
  systemFromData,
  profile ? "installed",
  ...
}:
let
  cfg = config.tentaflake;
  # This checkout owns its desired source (template, deployment fork or installer
  # copy). Input-based consumers import their own file, not this configuration.
  # Missing or Git-filtered desired state is never replaced by the public default.
  desiredSystem =
    if builtins.pathExists ./my-agents.nix then
      throw "tentaflake: manually migrate my-agents.nix to tentaflake.toml and remove the legacy file."
    else
      systemFromData { file = ./tentaflake.toml; };
in
{
  imports = [
    desiredSystem
  ]
  ++ lib.optionals (profile == "installed") [
    ./hardware-configuration.nix
  ];

  # ── OCI container backend (required for agent containers) ──
  virtualisation.oci-containers.backend = cfg.containerBackend;
  virtualisation.docker = lib.mkIf (profile == "installed" && cfg.containerBackend == "docker") {
    enable = true;
    autoPrune.enable = true;
  };

  # Docker group access is root-equivalent. Keep it only in the explicitly
  # compatibility-oriented dev profile; secure profiles use sudo for narrow
  # operator actions and never grant the daemon socket to the login user.
  users.users.${cfg.adminUser}.extraGroups = lib.optional (
    profile == "installed" && cfg.containerBackend == "docker" && cfg.security.profile == "dev"
  ) "docker";

  tentaflake.profile = profile;
  system.stateVersion = cfg.stateVersion;
}
