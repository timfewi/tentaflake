{
  config,
  pkgs,
  lib,
  modulesPath,
  mkHermesAgent,
  ...
}:

let
  liveAgents = import ./live-agents.nix { inherit mkHermesAgent; };
  piperVoices = pkgs.callPackage ../pkgs/piper-voices { };

  # Agent names defined in ./live-agents.nix. Kept in sync here so we can attach
  # boot-robustness overrides to their generated docker-hermes-<name> units.
  liveAgentNames = [
    "default"
    "research"
  ];
in
{
  imports = liveAgents ++ [
    ./firstboot.nix
    ../modules/piper-tts-server.nix
  ];

  # ── Auto-login root on TTY1 for firstboot wizard ──
  services.getty.autologinUser = "root";

  # ── Legacy VT on the live ISO ──
  # The firstboot wizard is gated on `[ "$(tty)" = /dev/tty1 ]` and writes
  # straight to /dev/tty1 (firstboot.nix / firstboot.sh). kmscon hands the login
  # a pty and owns the VT in graphics mode, so the wizard would neither trigger
  # nor be visible. The installed system turns kmscon on (modules/locale.nix).
  tentaflake.modernConsole.enable = false;
  # Same for the VT font — the firstboot wizard is a dialog TUI, see iso.nix.
  tentaflake.consoleFont = null;

  # ── Piper TTS with bundled voice from the ISO ──
  services.piper-tts-server = {
    enable = true;
    voiceName = "en_US-lessac-medium";
    voiceModel = "${piperVoices}/share/piper-voices/en_US-lessac-medium.onnx";
    voiceConfig = "${piperVoices}/share/piper-voices/en_US-lessac-medium.onnx.json";
  };

  # ── Tailscale for connectivity (user must auth) ──
  services.tailscale = {
    enable = true;
    openFirewall = true;
    extraUpFlags = [
      "--hostname=tentaflake-live"
      "--ssh"
    ];
  };

  # ── SSH for remote debugging ──
  # Key-only: Tailscale SSH (above) covers remote debugging on the live ISO,
  # so password auth stays off.
  services.openssh.enable = true;
  services.openssh.settings.PasswordAuthentication = false;

  # ── Boot robustness for the agent containers ──
  # The live ISO is ephemeral: the ~1.3 GB Hermes image is re-pulled into the
  # RAM overlay on every boot, so the docker-hermes-* units race the network.
  # Without this they pull before the link is up, fail ~5× fast, hit the
  # systemd start-limit, and wedge until you manually `reset-failed`+`restart`.
  #
  #   1. Order after network-online.target so the pull waits for connectivity.
  #   2. Disable the start-limit and retry every 10s, so a slow/late network
  #      just means it keeps trying until the pull succeeds — no manual steps.
  systemd.services = lib.genAttrs (map (n: "docker-hermes-${n}") liveAgentNames) (_: {
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    startLimitIntervalSec = 0; # [Unit] — disable the start-rate limit
    serviceConfig = {
      Restart = lib.mkForce "on-failure";
      RestartSec = lib.mkForce "10s";
    };
  });

  # ── Message of the day: installation instructions for every login ──
  users.motd = ''
    ╔══════════════════════════════════════════════════════════╗
    ║              Tentaflake Live Agent ISO                   ║
    ║                                                          ║
    ║  Hermes agents are running in Docker containers.          ║
    ║  Connect via:  sudo tailscale up                         ║
    ║                                                          ║
    ║  ── PERMANENT INSTALL ──                                  ║
    ║  To install Tentaflake permanently to a local disk:       ║
    ║    bash /etc/tentaflake/installer/installer.sh           ║
    ║                                                          ║
    ║  This will WIPE the target disk and install NixOS         ║
    ║  with the agent orchestration framework.                  ║
    ╚══════════════════════════════════════════════════════════╝
  '';

  # ── ISO-specific packages ──
  # The live ISO embeds the full repo and can also install to disk via
  # /etc/tentaflake/installer/installer.sh, so it must ship the same disk
  # toolchain as the installer ISO (parted, mkfs.*, nixos-install, …).
  # util-linux/cryptsetup cover the wipefs/swapoff/luks-close path for
  # re-partitioning a previously-used disk.
  environment.systemPackages = with pkgs; [
    piperVoices
    dialog # TUI wizard
    parted # partitioning (installer.sh)
    gptfdisk # sgdisk — zap/GPT repair
    dosfstools # mkfs.fat (EFI partition)
    e2fsprogs # mkfs.ext4 (root partition)
    util-linux # wipefs, swapoff, lsblk
    cryptsetup # close stray LUKS mappings before re-partitioning
    nixos-install-tools # nixos-install, nixos-generate-config
    git # flake operations during install
    jq # JSON utilities
  ];
}
