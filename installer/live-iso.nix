{
  config,
  pkgs,
  lib,
  modulesPath,
  repoRoot,
  ...
}:

# ────────────────────────────────────────────────────────────
# Tentaflake — Live Agent ISO
#
# Bootable ISO that auto-starts Hermes AI agents + Piper TTS.
# First boot: enter API keys via TUI wizard, or plug a USB
# labeled TENTAFLAKE_ENV (legacy: HERMES_ENV) with .env files
# to skip the wizard.
#
# Also usable as a standard installer — the full repo is
# embedded at /etc/tentaflake/ for installation.
# ────────────────────────────────────────────────────────────

{
  imports = [
    "${modulesPath}/installer/cd-dvd/iso-image.nix"
    ./live-profile.nix
  ];

  # ── ISO identity ──
  image.baseName = lib.mkForce "tentaflake-live";

  # ── Enable UEFI + USB hybrid boot ──
  isoImage.makeEfiBootable = true;
  isoImage.makeUsbBootable = true;

  # ── Override hostname (ISO-specific) ──
  networking.hostName = lib.mkForce "live-agent";

  # ── Embed the full repo source for installation ──
  environment.etc."tentaflake".source = repoRoot;

  # ── NetworkManager for connectivity ──
  networking.networkmanager.enable = true;

  # ── System state version ──
  system.stateVersion = config.tentaflake.stateVersion;
}
