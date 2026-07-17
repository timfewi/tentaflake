{ lib, ... }:
{
  imports = [
    ./options.nix
    ./boot.nix
    ./hardening.nix
    ./hive-research.nix
    ./locale.nix
    ./networking.nix
    ./nix-settings.nix
    ./packages.nix
    ./piper-tts-server.nix
    ./shell.nix
    ./ssh.nix
    ./tailscale.nix
    ./tentaflake-auditd.nix
    ./users.nix
  ];
}
