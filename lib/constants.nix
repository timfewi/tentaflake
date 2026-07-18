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

  # UID/GID the official hermes-agent image runs its `hermes` user as. The host
  # state dir + any mounted data must be owned by this so the container (which
  # runs as this uid) can write them. Owning host dirs to a *different* uid (e.g.
  # an auto-allocated NixOS system user) is the classic cause of PermissionError
  # on $HERMES_HOME. Override via mkHermesAgent's `containerUid`/`containerGid`.
  containerUid = 10000;
  containerGid = 10000;
}
