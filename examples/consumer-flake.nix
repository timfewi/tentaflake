# ─────────────────────────────────────────────────────────────────────
# examples/consumer-flake.nix — Minimal consumer config using tentaflake
#
# Keep deployment data in this consumer repository; the public input stays generic.
# Copy this example to <consumer>/flake.nix and the public commented template to
# <consumer>/tentaflake.toml. Review/edit that local TOML, then track it together
# with the flake and private supporting modules. Pin the input in flake.lock.
# Never put literal secrets in TOML: even a private consumer enters the Nix store.
#
#   git add flake.nix flake.lock tentaflake.toml
#   nix eval --json .#nixosConfigurations.agent-box.config.tentaflake.agentInstances
#   nix build --no-link .#nixosConfigurations.agent-box.config.system.build.toplevel
# Activation is a separate operator decision. See docs/08-agent-cli.md.
# ─────────────────────────────────────────────────────────────────────

{
  description = "My agent machine — powered by tentaflake";

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

      # ── Import tentaflake helpers ──
      systemFromData = tentaflake.lib.${system}.systemFromData;
      constants = tentaflake.lib.${system}.constants;

      # ── Shared specialArgs for all hosts ──
      specialArgs = {
        inherit
          self
          systemFromData
          constants
          ;
      };

      # ── mkHost: reusable NixOS system builder ──
      mkHost =
        {
          hostName,
          adminUser,
          modules,
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
            (
              { config, ... }:
              {
                tentaflake = {
                  configurationName = hostName;
                  hostName = nixpkgs.lib.mkDefault hostName;
                  inherit adminUser;
                  timeZone = nixpkgs.lib.mkDefault "UTC";
                  defaultLocale = nixpkgs.lib.mkDefault constants.defaultLocale;
                  consoleKeyMap = nixpkgs.lib.mkDefault constants.consoleKeyMap;
                  stateVersion = nixpkgs.lib.mkDefault constants.stateVersion;
                };
                # Input consumers do not import the root configuration.nix
                # which normally projects this bootstrap/desired setting.
                system.stateVersion = config.tentaflake.stateVersion;
              }
            )

            # 3. Agenix encrypted secrets
            agenix.nixosModules.age

            # 4. Home Manager (user-level dotfiles, shell, git, editor)
            home-manager.nixosModules.home-manager
            {
              home-manager = {
                useGlobalPkgs = true;
                useUserPackages = true;
                extraSpecialArgs = specialArgs;
                users.${adminUser} = import ./home.nix;
              };
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
            # Host broker, Research, worker and quota policy stays in Nix.
            ./machines/agent-box/host-policy.nix
            ./secrets/secrets.nix
            # The path is relative to THIS consumer flake, not the Tentaflake input.
            # Do not import tentaflake/configuration.nix or use its empty TOML as
            # fallback. Missing/untracked/ignored local TOML must fail evaluation.
            (systemFromData { file = ./tentaflake.toml; })
          ];
        };

        # ── Future machines: just add a new mkHost call ──
        # Give every additional managed system its own explicit desired source;
        # use separate consumer roots for the installed root-TOML CLI workflow.
        # another-host = mkHost {
        #   hostName = "another-host";
        #   adminUser = "alice";
        #   modules = [ ... ];
        # };
      };

      formatter.${system} = pkgs.nixfmt;
    };
}
