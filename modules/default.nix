{ lib, ... }:
{
  imports = [
    ./options.nix
    ./boot.nix
    ./hardening.nix
    ./hermes-auditd.nix
    ./locale.nix
    ./networking.nix
    ./nix-settings.nix
    ./packages.nix
    ./piper-tts-server.nix
    ./shell.nix
    ./tailscale.nix
    ./users.nix
  ];
}
