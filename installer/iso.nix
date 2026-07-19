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

  # ── Auto-login root on TTY1 ──
  # Root has no password (locked account) and there is no SSH or network
  # login: the only entry point is the autologin console on TTY1, which runs
  # the installer. If an install fails, its real cause is shown in the error
  # dialog (which includes the tail of /tmp/installer.log); no shell needed.
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

  # ── Legacy VT on the installer ISO ──
  # kmscon hands the login a pty, so the `tty` guard above would never see
  # /dev/tty1 and the installer would never auto-launch. The installed system
  # turns kmscon back on (see modules/locale.nix); dialog needs no braille.
  tentaflake.modernConsole.enable = false;
  # And no setfont either: dialog draws in ASCII (NCURSES_NO_UTF8_ACS=1 in
  # installer.sh), so the font swap buys nothing and its fbcon reconfiguration
  # flickers on some Intel panels.
  tentaflake.consoleFont = null;

  # ── No operator shell extras on the bare installer ──
  # TTY1 only ever runs installer.sh; a login banner / prompt / agent CLI would
  # just clutter the one-shot install flow (and there are no agents yet).
  tentaflake.shell.enable = false;

  # ── System state version ──
  system.stateVersion = config.tentaflake.stateVersion;
}
