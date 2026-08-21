{
  config,
  lib,
  pkgs,
  mkHermesAgent,
  mkZeroClawAgent,
  agentsFromData,
  profile ? "installed",
  ...
}:
let
  cfg = config.tentaflake;
  # ── Agent modules ──
  # Define agents in my-agents.nix (see my-agents.nix.example). Auto-imported when
  # present; git-track it (`git add my-agents.nix`) so the flake can evaluate it.
  # intersectAttrs passes each builder only if the function asks for it, so older
  # my-agents.nix files that take just `{ mkHermesAgent }` keep working.
  myAgents = lib.optionals (builtins.pathExists ./my-agents.nix) (
    let
      f = import ./my-agents.nix;
    in
    f (lib.intersectAttrs (lib.functionArgs f) { inherit mkHermesAgent mkZeroClawAgent; })
  );

  # agents.json — declarative non-Nix input. Additive to my-agents.nix (both may
  # coexist); git-tracked and intentionally secret-free.
  dataAgents = lib.optionals (builtins.pathExists ./agents.json) (agentsFromData {
    file = ./agents.json;
    inherit mkHermesAgent mkZeroClawAgent;
  });
in
{
  imports =
    myAgents
    ++ dataAgents
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
