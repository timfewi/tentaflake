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
    in
    {
      nixosModules.default = import ./modules/default.nix;

      formatter.${system} = pkgs.nixfmt-tree;

      # Toolchain plus the entry banner; see lib/devshell.nix.
      devShells.${system}.default = import ./lib/devshell.nix { inherit pkgs; };

      # `nix flake check --no-build` evaluates these; building them is explicit.
      checks.${system} = {
        ${hostName} = self.nixosConfigurations.${hostName}.config.system.build.toplevel;
        tentaflake-broker = self.packages.${system}.tentaflake-broker;
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
            };
          }
          self.nixosModules.default
          ./configuration.nix
        ];
      };

      packages.${system}.tentaflake-broker = pkgs.callPackage ./pkgs/tentaflake-broker { };
    };
}
