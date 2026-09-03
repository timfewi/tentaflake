{
  description = "Generic NixOS flake template for running isolated Hermes and ZeroClaw agents on one headless machine";

  inputs = {
    # Tracks nixos-unstable, pinned to an exact revision by the committed flake.lock
    # (so builds are reproducible — run `nix flake update` to bump deliberately).
    # The pinned revision provides the container and Rust toolchains used by the
    # host and the workspace CLI.
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
        defaultLocale
        consoleKeyMap
        stateVersion
        ;

      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      inherit (nixpkgs) lib;

      # Root of the repo — used by installer ISO to embed source
      repoRoot = ./.;

      # ── Template constants ──
      constants = import ./lib/constants.nix;

      # ── Shared agent builders ──
      inherit ((import ./lib { inherit pkgs lib; })) mkHermesAgent;
      inherit ((import ./lib { inherit pkgs lib; })) mkZeroClawAgent;
      inherit ((import ./lib { inherit pkgs lib; })) agentsFromData;

      # Module set imported by external consumers and built-in hosts
      tentaflakeModules = import ./modules/default.nix;

      # Contributor E2E tooling can move ahead of the nixpkgs package while
      # remaining source- and dependency-hash pinned.
      devcontainerCli = import ./lib/devcontainer-cli.nix { inherit pkgs; };

      # Security tools use the locked nixpkgs revisions; rule updates are
      # separate, reviewable hash bumps instead of live registry downloads.
      securityTools = pkgs.symlinkJoin {
        name = "tentaflake-security-tools";
        paths = [
          pkgs.osv-scanner
          pkgs.semgrep
        ];
      };
      semgrepRules = pkgs.fetchFromGitHub {
        owner = "semgrep";
        repo = "semgrep-rules";
        rev = "40b8c63f75dc7c22c8a77482d73bfb864b146f7e";
        hash = "sha256-VtPavzFGDmRzdG9wTFc+yp7TbI1gT1/IaF//K1m3OT0=";
      };

      # Shared specialArgs — no host-specific params here
      baseSpecialArgs = {
        inherit
          inputs
          self
          mkHermesAgent
          mkZeroClawAgent
          agentsFromData
          repoRoot
          constants
          ;
      };
    in
    {
      # ── Exported module set ──
      nixosModules = {
        default = tentaflakeModules;

        # Installer and optional profiles/integrations are exported explicitly;
        # none of them enlarge nixosModules.default.
        installer = import ./installer/iso.nix;
        editor = import ./modules/optional/editor.nix;
        hiveResearch = import ./modules/optional/hive-research.nix;
        piperTts = import ./modules/optional/piper-tts-server.nix;
        observability = import ./modules/profiles/observability.nix;
        falco = import ./modules/profiles/falco.nix;
      };

      # ── Exported helpers ──
      lib.${system} = {
        inherit
          mkHermesAgent
          mkZeroClawAgent
          agentsFromData
          constants
          ;
      };

      # ── Formatting ──
      formatter.${system} = nixpkgs.legacyPackages.${system}.nixfmt-tree;

      # ── Dev shell for contributors ──
      # Toolchain plus the entry banner; see lib/devshell.nix.
      devShells.${system}.default = import ./lib/devshell.nix { inherit pkgs; };

      # ── Checks (validates nixosConfigurations build) ──
      checks.${system} = {
        ${hostName} = self.nixosConfigurations.${hostName}.config.system.build.toplevel;
        tentaflake-cli = self.packages.${system}.tentaflake-cli;
        tentaflake-broker = self.packages.${system}.tentaflake-broker;
        tentaflake-worker = self.packages.${system}.tentaflake-worker;
        tentaflake-worker-image = self.packages.${system}.tentaflake-worker-image;
        devcontainer-cli = self.packages.${system}.devcontainer-cli;
        golden-eval-schema =
          pkgs.runCommand "tentaflake-golden-eval-schema"
            {
              nativeBuildInputs = [ pkgs.python3 ];
            }
            ''
              test -f ${./tests}/test_golden_eval_runner.py
              python3 -B -m unittest discover \
                -s ${./tests} \
                -p 'test_golden_eval_runner.py' \
                -v
              touch "$out"
            '';
        image-pinning = import ./lib/pinnedImage-test.nix { inherit pkgs; };
        module-evaluation =
          assert import ./tests/module-eval.nix { nixpkgsPath = nixpkgs.outPath; };
          pkgs.runCommand "tentaflake-module-evaluation" { } "touch $out";
        sui-attestation-signing-vector =
          pkgs.runCommand "tentaflake-sui-attestation-signing-vector"
            {
              nativeBuildInputs = [ pkgs.python3 ];
            }
            ''
              python3 -B ${./scripts/verify-sui-attestation-vector.py} \
                ${./integrations/sui-agent-attestation/test-vectors/signing-payload-v1.json}
              touch "$out"
            '';

        # VM integration test: boots the host and asserts the runtime path
        # (Rust CLI, status, and agent unit/user/state dir).
        vm-integration = pkgs.testers.runNixOSTest (
          import ./tests/integration.nix { inherit self mkHermesAgent mkZeroClawAgent; }
        );
      };

      # ── tentaflake: Installed system, consumes my-agents.nix ──
      # Attr name is `hostName` so it always matches what `tentaflake rebuild`
      # passes to `nixos-rebuild --flake .#<host>`.
      nixosConfigurations.${hostName} = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = baseSpecialArgs // {
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
              adminShell = "${pkgs.zsh}/bin/zsh";
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
              shell = {
                enable = true;
                # Interactive extras (all opt-in; on here for the built-in host).
                zsh.enable = true;
                zoxide.enable = true;
                lazygit.enable = true;
                tmux.enable = true;
              };
            };
          }
          self.nixosModules.default
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
          {
            # The ISO installs an agent host but does not run untrusted agents
            # itself, so it does not need the balanced gVisor capsule closure.
            tentaflake.security.profile = "dev";
          }
          self.nixosModules.default
          ./configuration.nix
          ./installer/iso.nix
        ];
      };

      # ── Convenience packages ──
      packages.${system} = rec {
        devcontainer-cli = devcontainerCli;
        security-tools = securityTools;
        semgrep-rules = semgrepRules;
        tentaflake-cli = pkgs.callPackage ./pkgs/tentaflake-cli { };
        tentaflake-broker = pkgs.callPackage ./pkgs/tentaflake-broker { };
        tentaflake-worker = pkgs.callPackage ./pkgs/tentaflake-worker { };
        tentaflake-worker-image = pkgs.callPackage ./pkgs/tentaflake-worker/image.nix { };
        installer-iso = self.nixosConfigurations.installer-iso.config.system.build.isoImage;
        piper-voices = pkgs.callPackage ./pkgs/piper-voices { };
      };
    };
}
