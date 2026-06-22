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

    # Optional: uncomment for Niri + Noctalia desktop (NNN stack)
    # noctalia = {
    #   url = "github:noctalia-dev/noctalia";
    #   inputs.nixpkgs.follows = "nixpkgs";
    # };

    # Optional: uncomment for agenix encrypted secrets
    # agenix = {
    #   url = "github:ryantm/agenix";
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

      # ── Template constants (stateVersion, locale defaults) ──
      constants = import ./lib/constants.nix;

      # ── Configurable parameters — override when forking ──
      params = {
        hostName = constants.hostName;
        adminUser = constants.adminUser;
        adminDescription = constants.adminDescription;
        adminShell = constants.adminShell;
        timeZone = "UTC";
        defaultLocale = constants.defaultLocale;
        consoleKeyMap = constants.consoleKeyMap;
        stateVersion = constants.stateVersion;
      };

      # ── Shared mkHermesAgent helper ──
      mkHermesAgent = (import ./lib { inherit pkgs lib; }).mkHermesAgent;

      # Passed to all NixOS modules via specialArgs
      specialArgs = {
        inherit
          inputs
          self
          params
          mkHermesAgent
          repoRoot
          constants
          ;
      };
    in
    {
      formatter.${system} = nixpkgs.legacyPackages.${system}.nixfmt;

      # ── Single example host ──
      nixosConfigurations.agent-host = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = specialArgs // {
          isLiveISO = false;
        };
        modules = [
          # Optional: import hermes-agent NixOS module for advanced use
          # inputs.hermes-agent.nixosModules.default

          # Optional: uncomment for home-manager support
          # inputs.home-manager.nixosModules.home-manager

          # Optional: uncomment for agenix encrypted secrets
          # inputs.agenix.nixosModules.age

          ./configuration.nix
        ];
      };

      # ── Bootable installer ISO ──
      # Build with: nix build .#installer-iso
      # Embeds entire repo at /etc/tentaflake/ on the ISO
      nixosConfigurations.installer-iso = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = specialArgs // {
          isLiveISO = false;
        };
        modules = [
          ./installer/iso.nix
        ];
      };

      # ── Live agent ISO (Hermes agents + Piper TTS out of the box) ──
      # Build with: nix build .#live-agent-iso
      # Boot → enter API keys → agents running
      nixosConfigurations.live-agent = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = specialArgs // {
          isLiveISO = true;
        };
        modules = [
          ./installer/live-iso.nix
        ];
      };

      # ── Convenience packages ──
      packages.${system} = {
        installer-iso = self.nixosConfigurations.installer-iso.config.system.build.isoImage;
        live-agent-iso = self.nixosConfigurations.live-agent.config.system.build.isoImage;
        piper-voices = pkgs.callPackage ./pkgs/piper-voices { };
      };
    };
}
