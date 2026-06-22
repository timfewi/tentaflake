{
  config,
  pkgs,
  lib,
  modulesPath,
  ...
}:

let
  mkHermesAgent = import ../lib/mkHermesAgent.nix { inherit pkgs lib; };
  liveAgents = import ./live-agents.nix { inherit mkHermesAgent; };
  piperVoices = pkgs.callPackage ../pkgs/piper-voices { };
in
{
  imports = liveAgents ++ [
    ../modules/hermes-firstboot.nix
    ../modules/piper-tts-server.nix
  ];

  # ── Auto-login root on TTY1 for firstboot wizard ──
  services.getty.autologinUser = "root";

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
  services.openssh.enable = true;
  services.openssh.settings.PasswordAuthentication = true;

  # ── ISO-specific packages ──
  environment.systemPackages = with pkgs; [
    piperVoices
    dialog
  ];
}
