{
  researchFlake ? null,
  ...
}:
{
  imports = [
    ./agent-instances.nix
    ../adapters/pi-durable/options.nix
    ./options.nix
    ./boot.nix
    ./backup.nix
    ./broker.nix
    ./control.nix
    ./hardening.nix
    ./host-api.nix
    ./image-provenance.nix
    ./integrations.nix
    ./locale.nix
    ./management.nix
    ./networking.nix
    ./nix-settings.nix
    ./packages.nix
    ./security.nix
    ./shell.nix
    ./ssh.nix
    ./tailscale.nix
    ./users.nix
    ./worker.nix
    ./workload-mapping.nix
    ./workspace-quota.nix
  ]
  ++ (if researchFlake == null then [ ] else [ (import ./research.nix { inherit researchFlake; }) ]);
}
