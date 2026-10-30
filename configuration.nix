{
  config,
  lib,
  agentsFromData,
  profile ? "installed",
  ...
}:
let
  cfg = config.tentaflake;
  # agents.json is the only instance input; a missing file defines an empty host.
  # Never evaluate removed Nix declarations or silently drop existing instances.
  dataAgents =
    if builtins.pathExists ./my-agents.nix then
      throw "tentaflake: my-agents.nix is no longer supported. Manually migrate declarations to agents.json and host policy to Nix modules, preserving names, state paths and ownership; then remove my-agents.nix. No automatic migration is performed."
    else
      lib.optionals (builtins.pathExists ./agents.json) (agentsFromData {
        file = ./agents.json;
      });
in
{
  imports =
    dataAgents
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
