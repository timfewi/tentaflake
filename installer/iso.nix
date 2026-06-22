{
  config,
  pkgs,
  lib,
  modulesPath,
  repoRoot,
  ...
}:

# ────────────────────────────────────────────────────────────
# Tentaflake — Installer ISO
# Bootable live ISO that auto-launches installer.sh on TTY1.
# Embeds the full repo at /etc/tentaflake/
# ────────────────────────────────────────────────────────────

{
  imports = [ "${modulesPath}/installer/cd-dvd/iso-image.nix" ];

  # The actual ISO filename — must force because iso-image module sets it in its config block
  image.baseName = lib.mkForce "tentaflake";

  # Enable UEFI boot (required for modern PCs) and USB hybrid mode (required for dd to USB)
  isoImage.makeEfiBootable = true;
  isoImage.makeUsbBootable = true;

  # ── Embed the full repo source ──
  environment.etc."tentaflake".source = repoRoot;

  # ── Packages needed by the installer ──
  environment.systemPackages = with pkgs; [
    dialog # TUI wizard (whiptail-compatible)
    jq # JSON utils (future use)
    parted # Partitioning
    gptfdisk # GPT disk tools
    dosfstools # mkfs.fat
    e2fsprogs # mkfs.ext4
    nixos-install-tools # nixos-install, nixos-generate-config
    git # For potential flake operations
  ];

  # ── NetworkManager for install-time connectivity ──
  networking.networkmanager.enable = true;

  # ── SSH access for debugging (optional) ──
  services.openssh.enable = true;
  services.openssh.settings.PasswordAuthentication = true;

  # ── Auto-login root on TTY1 ──
  services.getty.autologinUser = "root";

  # ── Run installer on TTY1 login ──
  programs.bash.interactiveShellInit = ''
    if [ -z "$INSTALLER_RUN" ] && [ "$(tty 2>/dev/null)" = "/dev/tty1" ]; then
      export INSTALLER_RUN=1
      echo ""
      echo "=== Tentaflake Installer ==="
      echo ""
      /etc/tentaflake/installer/installer.sh
      exit
    fi
  '';

  # ── System state version ──
  system.stateVersion = "26.05";
}
