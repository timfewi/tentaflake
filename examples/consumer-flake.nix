# ─────────────────────────────────────────────────────────────────────
# examples/consumer-flake.nix — Minimal consumer config using tentaflake
#
# This is the RECOMMENDED way to use tentaflake:
# your private config imports it as a flake input.
#
#   git clone <your-private-repo>
#   cd nixos-tentaflake-config
#   sudo nixos-rebuild switch --flake .#<hostname>
# ─────────────────────────────────────────────────────────────────────

{
  description = "My Hermes agent machine — powered by tentaflake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    tentaflake = {
      url = "github:timfewi/tentaflake";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    agenix = {
      url = "github:ryantm/agenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      tentaflake,
      home-manager,
      agenix,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      lib = nixpkgs.lib;

      # ── Import tentaflake helpers ──
      mkHermesAgent = tentaflake.lib.${system}.mkHermesAgent;
      constants = tentaflake.lib.${system}.constants;

      # ── Shared specialArgs for all hosts ──
      specialArgs = {
        inherit self mkHermesAgent constants;
      };

      # ── mkHost: reusable NixOS system builder ──
      mkHost =
        {
          hostName,
          adminUser,
          modules,
          extraHomeModules ? [ ],
        }:
        nixpkgs.lib.nixosSystem {
          inherit system;
          specialArgs = specialArgs // {
            inherit hostName;
          };

          modules = [
            # 1. Tentaflake base modules (boot, locale, networking, etc.)
            tentaflake.nixosModules.default

            # 2. Tentaflake config — set your options here
            {
              tentaflake = {
                inherit hostName;
                adminUser = adminUser;
                timeZone = "UTC";
                defaultLocale = constants.defaultLocale;
                consoleKeyMap = constants.consoleKeyMap;
                stateVersion = constants.stateVersion;
              };
            }

            # 3. Agenix encrypted secrets
            agenix.nixosModules.age

            # 4. Home Manager (user-level dotfiles, shell, git, editor)
            home-manager.nixosModules.home-manager
            {
              home-manager.useGlobalPkgs = true;
              home-manager.useUserPackages = true;
              home-manager.extraSpecialArgs = specialArgs;
              home-manager.users.${adminUser} = import ./home.nix;
            }
          ]
          # 5. Machine-specific modules
          ++ modules;
        };
    in
    {
      nixosConfigurations = {
        # ── First machine ──
        agent-box = mkHost {
          hostName = "agent-box";
          adminUser = "alice";
          modules = [
            ./machines/agent-box/default.nix
            ./machines/agent-box/hardware-configuration.nix
            ./machines/agent-box/agents.nix
            ./secrets/secrets.nix
          ];
        };

        # ── Future machines: just add a new mkHost call ──
        # another-host = mkHost {
        #   hostName = "another-host";
        #   adminUser = "alice";
        #   modules = [ ... ];
        # };
      };

      formatter.${system} = pkgs.nixfmt;
    };
}
