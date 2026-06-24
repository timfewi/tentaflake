{
  description = "Generic NixOS flake template for running multiple isolated Hermes agents on one headless machine";

  inputs = {
    # Tracks nixos-unstable, pinned to an exact revision by the committed flake.lock
    # (so builds are reproducible — run `nix flake update` to bump deliberately).
    # Unstable is required here: it is the only channel that currently provides BOTH
    # a non-vulnerable docker (29.x) AND Go >= 1.25 (needed by hermes-auditd's
    # modernc.org/sqlite). The 25.11 stable ships docker 28.5.2, flagged insecure.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Optional: use hermes-agent NixOS module for single-agent setups or container images
    hermes-agent = {
      url = "github:NousResearch/hermes-agent";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Neovim distribution (consumed by modules/editor.nix → tentaflake.editor.nvf)
    nvf = {
      url = "github:NotAShelf/nvf";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Optional: uncomment for home-manager support
    # home-manager = {
    #   url = "github:nix-community/home-manager";
    #   inputs.nixpkgs.follows = "nixpkgs";
    # };

    # Optional: uncomment for agenix encrypted secrets
    # agenix = {
    #   url = "github:ryantm/agenix";
    #   inputs.nixpkgs.follows = "nixpkgs";
    # };

    # Optional: uncomment for treefmt-nix (format checking in CI)
    # treefmt-nix = {
    #   url = "github:numtide/treefmt-nix";
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
      inherit (import ./lib/constants.nix)
        hostName
        adminUser
        adminDescription
        adminShell
        defaultLocale
        consoleKeyMap
        stateVersion
        ;

      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      lib = nixpkgs.lib;

      # Root of the repo — used by installer ISO to embed source
      repoRoot = ./.;

      # ── Template constants ──
      constants = import ./lib/constants.nix;

      # ── Shared mkHermesAgent helper ──
      mkHermesAgent = (import ./lib { inherit pkgs lib; }).mkHermesAgent;

      # Module set imported by external consumers and built-in hosts
      tentaflakeModules = import ./modules/default.nix;

      # Shared specialArgs — no host-specific params here
      baseSpecialArgs = {
        inherit
          inputs
          self
          mkHermesAgent
          repoRoot
          constants
          ;
      };
    in
    {
      # ── Exported module set ──
      nixosModules.default = tentaflakeModules;

      # Also export installer-specific modules so consumers can compose them
      nixosModules.installer = import ./installer/iso.nix;
      nixosModules.live = import ./installer/live-iso.nix;

      # Optional Neovim (nvf) module. Kept out of nixosModules.default because it
      # needs the `nvf` flake input; consumers add that input and import this.
      nixosModules.editor = import ./modules/editor.nix;

      # ── Exported helpers ──
      lib.${system} = { inherit mkHermesAgent constants; };

      # ── Formatting ──
      formatter.${system} = nixpkgs.legacyPackages.${system}.nixfmt-tree;

      # ── Dev shell for contributors ──
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          nixfmt-rfc-style
          statix
          deadnix
          nil
          gotools
          golangci-lint
          shellcheck
        ];
      };

      # ── Checks (validates nixosConfigurations build) ──
      checks.${system} = {
        agent-host = self.nixosConfigurations.agent-host.config.system.build.toplevel;
        hermes-auditd = self.packages.${system}.hermes-auditd;
      };

      # ── agent-host: Installed system, consumes my-agents.nix ──
      nixosConfigurations.agent-host = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = baseSpecialArgs // {
          profile = "installed";
        };
        modules = [
          {
            tentaflake.hostName = hostName;
            tentaflake.adminUser = adminUser;
            tentaflake.adminDescription = adminDescription;
            tentaflake.adminShell = "${pkgs.zsh}/bin/zsh";
            tentaflake.timeZone = "UTC";
            tentaflake.defaultLocale = defaultLocale;
            tentaflake.consoleKeyMap = consoleKeyMap;
            tentaflake.stateVersion = stateVersion;
            tentaflake.allowUnfree = false;
            tentaflake.boot.enable = true;
            tentaflake.hardening.enable = true;
            tentaflake.locale.enable = true;
            tentaflake.networking.enable = true;
            tentaflake.nixSettings.enable = true;
            tentaflake.packages.enable = true;
            tentaflake.users.enable = true;
            tentaflake.tailscale.enable = true;
            tentaflake.shell.enable = true;
            # Interactive extras (all opt-in; on here for the built-in host).
            tentaflake.shell.zsh.enable = true;
            tentaflake.shell.zoxide.enable = true;
            tentaflake.shell.lazygit.enable = true;
            tentaflake.shell.tmux.enable = true;
            tentaflake.editor.nvf.enable = true;
            # Audit daemon: records agent filesystem activity for `hermes top`.
            # watchDirs auto-derives from the agents defined in my-agents.nix.
            tentaflake.hermes-auditd.enable = true;
          }
          self.nixosModules.default
          self.nixosModules.editor
          ./configuration.nix
        ];
      };

      # ── installer-iso: Bare installer, embeds repo, runs installer.sh ──
      nixosConfigurations.installer-iso = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = baseSpecialArgs // {
          profile = "installer";
        };
        modules = [
          self.nixosModules.default
          ./configuration.nix
          ./installer/iso.nix
        ];
      };

      # ── live-agent: Boot-and-run appliance, auto-starts agents + Piper ──
      nixosConfigurations.live-agent = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = baseSpecialArgs // {
          profile = "live";
        };
        modules = [
          {
            tentaflake.hostName = "live-agent";
            tentaflake.adminUser = adminUser;
            tentaflake.adminDescription = adminDescription;
            tentaflake.adminShell = "/run/current-system/sw/bin/bash";
            tentaflake.timeZone = "UTC";
            tentaflake.defaultLocale = defaultLocale;
            tentaflake.consoleKeyMap = consoleKeyMap;
            tentaflake.stateVersion = stateVersion;
            tentaflake.allowUnfree = false;
            tentaflake.boot.enable = true;
            tentaflake.hardening.enable = true;
            tentaflake.locale.enable = true;
            tentaflake.networking.enable = true;
            tentaflake.nixSettings.enable = true;
            tentaflake.packages.enable = true;
            tentaflake.users.enable = true;
            tentaflake.tailscale.enable = true;
            # Shell extras are useful on the live ISO too, but the live profile
            # ships its own static users.motd — disable the dynamic banner so
            # operators don't see two banners stacked on every login.
            tentaflake.shell.enable = true;
            tentaflake.shell.motd.enable = false;
            tentaflake.shell.tmux.enable = true;
            # Audit daemon on too, so `hermes top` works on the live appliance —
            # watchDirs auto-derives from the live agents (default + research).
            tentaflake.hermes-auditd.enable = true;
          }
          self.nixosModules.default
          ./configuration.nix
          ./installer/live-iso.nix
        ];
      };

      # ── Convenience packages ──
      packages.${system} = {
        hermes-auditd = pkgs.callPackage ./pkgs/hermes-auditd { };
        installer-iso = self.nixosConfigurations.installer-iso.config.system.build.isoImage;
        live-agent-iso = self.nixosConfigurations.live-agent.config.system.build.isoImage;
        piper-voices = pkgs.callPackage ./pkgs/piper-voices { };
      };
    };
}
