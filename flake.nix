{
  description = "Tentaflake: a NixOS host for isolated, UI-built agent environments";

  inputs = {
    # Tracks nixos-unstable, pinned to an exact revision by the committed flake.lock
    # (run `nix flake update` to bump deliberately).
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
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
        defaultLocale
        consoleKeyMap
        stateVersion
        ;

      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      host = self.nixosConfigurations.${hostName};

      # The provider-agnostic VPS disk image of the host: GPT with a BIOS boot
      # partition, an ESP and the root file system (hardware-configuration.nix).
      diskImage =
        format:
        import "${nixpkgs}/nixos/lib/make-disk-image.nix" {
          inherit (host) config pkgs;
          inherit (nixpkgs) lib;
          inherit format;
          baseName = "tentaflake";
          partitionTableType = "hybrid";
          bootSize = "1024M";
          copyChannel = false;
        };
    in
    {
      nixosModules.default = import ./modules/default.nix;

      formatter.${system} = pkgs.nixfmt-tree;

      # Toolchain plus the entry banner; see lib/devshell.nix.
      devShells.${system}.default = import ./lib/devshell.nix { inherit pkgs; };

      # `nix flake check --no-build` evaluates these; building them is explicit.
      checks.${system} = {
        ${hostName} = host.config.system.build.toplevel;
        inherit (self.packages.${system})
          tentaflake-broker
          tentaflake-control
          tentaflake-ui
          image-raw
          image-qcow2
          ;
      };

      nixosConfigurations.${hostName} = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = {
          inherit inputs self;
          profile = "installed";
        };
        modules = [
          {
            tentaflake = {
              inherit
                hostName
                adminUser
                adminDescription
                defaultLocale
                consoleKeyMap
                stateVersion
                ;
              timeZone = "UTC";
              allowUnfree = false;
              boot.enable = true;
              hardening.enable = true;
              locale.enable = true;
              networking.enable = true;
              nixSettings.enable = true;
              packages.enable = true;
              users.enable = true;
              tailscale.enable = true;
              control.enable = true;
              # The login screen shows the setup notice; kmscon has no issue file.
              modernConsole.enable = false;
            };
          }
          self.nixosModules.default
          ./configuration.nix
        ];
      };

      packages.${system} = {
        tentaflake-broker = pkgs.callPackage ./pkgs/tentaflake-broker { };
        tentaflake-control = pkgs.callPackage ./pkgs/tentaflake-control { };
        tentaflake-ui = pkgs.callPackage ./pkgs/tentaflake-ui { };
        # Building them is explicit (a VM run each); see the README.
        image-raw = diskImage "raw";
        image-qcow2 = diskImage "qcow2-compressed";
      };
    };
}
