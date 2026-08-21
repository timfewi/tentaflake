{ lib, ... }:
{
  imports = [
    ./options.nix
    ./boot.nix
    ./backup.nix
    ./broker.nix
    ./hardening.nix
    ./image-provenance.nix
    ./locale.nix
    ./networking.nix
    ./nix-settings.nix
    ./packages.nix
    ./security.nix
    ./shell.nix
    ./ssh.nix
    ./tailscale.nix
    ./users.nix
    ./worker.nix
    ./workspace-quota.nix
  ];
}
