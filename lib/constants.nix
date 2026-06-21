# ────────────────────────────────────────────────────────────
# constants.nix — Template-wide constants
#
# Single source of truth for values shared across modules.
# Override these in your fork's flake.nix params if needed.
# ────────────────────────────────────────────────────────────

{
  # NixOS release this template targets
  stateVersion = "26.05";

  # Locale defaults
  defaultLocale = "en_US.UTF-8";
  consoleKeyMap = "us";

  # Host name
  hostName = "agent-host";

  # Default Admin name
  adminUser = "admin";

  # Default shell
  # NOTE: Override in your flake.nix params to ${pkgs.bash}/bin/bash for portability
  adminShell = "/run/current-system/sw/bin/bash";

  # Default user description
  adminDescription = "System Administrator";

  # Reserved: default image for mkHermesAgent (not wired yet)
  # hermesImage = "ghcr.io/nousresearch/hermes-agent:latest";
}
