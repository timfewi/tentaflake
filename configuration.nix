{
  config,
  lib,
  pkgs,
  mkHermesAgent,
  mkZeroClawAgent,
  mkOpenCodeAgent,
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
    f (lib.intersectAttrs (lib.functionArgs f) { inherit mkHermesAgent mkZeroClawAgent mkOpenCodeAgent; })
  );

  # agents.json — non-Nix config written by the `tentaflake` setup wizard, for
  # non-developers. Additive to my-agents.nix (both may coexist); git-tracked
  # (no secrets — see lib/agentsFromData.nix).
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
  virtualisation.docker = lib.mkIf (cfg.containerBackend == "docker") {
    enable = true;
    autoPrune.enable = true;
  };

  # ── Admin user in the docker group for CLI container management ──
  # (podman is rootless/daemonless and needs no group)
  users.users.${cfg.adminUser}.extraGroups = lib.optional (cfg.containerBackend == "docker") "docker";

  system.stateVersion = cfg.stateVersion;
}
