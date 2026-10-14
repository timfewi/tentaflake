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

  # TTY font. The kernel's built-in font has only 256 CP437-era glyphs, so box
  # drawing (btop, the banner separator) renders as garbage on a bare console.
  # Terminus covers those; bump to ter-v32n on HiDPI panels.
  consoleFont = "ter-v16n";

  # Host name. Doubles as the flake attribute name for the built-in host
  # (`nixos-rebuild --flake .#<hostName>`), so the two must not drift apart.
  hostName = "tentaflake";

  # Default Admin name
  adminUser = "user";

  # Default shell
  # NOTE: Override via tentaflake.adminShell to ${pkgs.bash}/bin/bash for portability
  adminShell = "/run/current-system/sw/bin/bash";

  # Default user description
  adminDescription = "System Administrator";
}
