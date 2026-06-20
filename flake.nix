{
  description = "Generic NixOS flake template for running multiple isolated Hermes agents on one headless machine";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Optional: use hermes-agent NixOS module for single-agent setups or container images
    hermes-agent = {
      url = "github:NousResearch/hermes-agent";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Optional: uncomment for home-manager support
    # home-manager = {
    #   url = "github:nix-community/home-manager";
    #   inputs.nixpkgs.follows = "nixpkgs";
    # };
  };

  outputs =
    {
      self,
      nixpkgs,
      ...
    }@inputs:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      lib = nixpkgs.lib;

      # Root of the repo — used by installer ISO to embed source
      repoRoot = ./.;

      # ── Configurable parameters — override when forking ──
      params = {
        hostName = "agent-host";
        adminUser = "admin";
        adminDescription = "System Administrator";
        adminShell = "${pkgs.bash}/bin/bash";
        timeZone = "UTC";
        defaultLocale = "en_US.UTF-8";
        consoleKeyMap = "us";
        stateVersion = "26.05";
      };

      # ── Shared mkHermesAgent helper ──
      mkHermesAgent = import ./lib { inherit pkgs lib; };

      # Passed to all NixOS modules via specialArgs
      specialArgs = {
        inherit
          inputs
          self
          params
          mkHermesAgent
          repoRoot
          ;
      };
    in
    {
      formatter.${system} = nixpkgs.legacyPackages.${system}.nixfmt;

      # ── Single example host ──
      nixosConfigurations.agent-host = nixpkgs.lib.nixosSystem {
        inherit system specialArgs;
        modules = [
          # Optional: import hermes-agent NixOS module for advanced use
          # inputs.hermes-agent.nixosModules.default

          # Optional: uncomment for home-manager support
          # inputs.home-manager.nixosModules.home-manager

          ./configuration.nix
        ];
      };

      # ── Bootable installer ISO ──
      # Build with: nix build .#installer-iso
      # Embeds entire repo at /etc/nixos-agent-orchestration/ on the ISO
      nixosConfigurations.installer-iso = nixpkgs.lib.nixosSystem {
        inherit system specialArgs;
        modules = [
          ./installer/iso.nix
        ];
      };

      # ── Convenience package: point to the ISO image file ──
      packages.${system}.installer-iso =
        self.nixosConfigurations.installer-iso.config.system.build.isoImage;
    };
}
