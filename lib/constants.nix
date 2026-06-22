# ────────────────────────────────────────────────────────────
# constants.nix — Template-wide constants
#
# Single source of truth for default values shared across modules.
# Override these via tentaflake.* options in your host configuration.
# ────────────────────────────────────────────────────────────

{
  # NixOS state version for stateful data compatibility. Set once at first install
  # and normally left unchanged across upgrades.
  stateVersion = "26.05";

  # Locale defaults
  defaultLocale = "en_US.UTF-8";
  consoleKeyMap = "us";

  # Host name
  hostName = "agent-host";

  # Default Admin name
  adminUser = "admin";

  # Default shell
  # NOTE: Override via tentaflake.adminShell to ${pkgs.bash}/bin/bash for portability
  adminShell = "/run/current-system/sw/bin/bash";

  # Default user description
  adminDescription = "System Administrator";

  # Reserved: default image for mkHermesAgent (not wired yet)
  # hermesImage = "ghcr.io/nousresearch/hermes-agent:latest";
}
