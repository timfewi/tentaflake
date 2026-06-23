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
    gptfdisk # GPT disk tools (sgdisk)
    dosfstools # mkfs.fat
    e2fsprogs # mkfs.ext4
    nixos-install-tools # nixos-install, nixos-generate-config
    git # For potential flake operations
    cryptsetup # luksClose for previously-encrypted disks
    lvm2 # dmsetup, vgchange for LVM on previously-used disks
  ];

  # ── NetworkManager for install-time connectivity ──
  networking.networkmanager.enable = true;

  # ── SSH access for debugging (optional) ──
  services.openssh.enable = true;
  services.openssh.settings.PasswordAuthentication = true;
  services.openssh.settings.PermitRootLogin = lib.mkForce "yes";
  services.openssh.settings.PermitEmptyPasswords = true;

  # ── Empty root password so a debug console is reachable ──
  # The installer loops on TTY1 (autologin → installer.sh → relogin), so the
  # only way to a shell to read /tmp/installer.log is another VT (Ctrl+Alt+F2)
  # or SSH. An unset root password is locked and rejects login; force it empty
  # so "root" + <enter> works. This is a throwaway live installer ISO — no
  # secrets, no persistence — so an empty root password is acceptable here.
  users.users.root.initialHashedPassword = lib.mkForce "";

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
  system.stateVersion = config.tentaflake.stateVersion;
}
