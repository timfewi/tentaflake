{
  description = "Agent-agnostic NixOS flake template for isolated agents on one headless machine";

  inputs = {
    tentaflake-research.url = "github:timfewi/tentaflake-research";
    # Tracks nixos-unstable, pinned to an exact revision by the committed flake.lock
    # (so builds are reproducible — run `nix flake update` to bump deliberately).
    # The pinned revision provides the container and Rust toolchains used by the
    # host and the workspace CLI.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

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

      # ── Public JSON loading and runtime facts ──
      inherit ((import ./lib { inherit pkgs lib; })) agentsFromData runtimeCatalog;

      # Module set imported by external consumers and built-in hosts
      tentaflakeModules = import ./modules/default.nix { researchFlake = inputs.tentaflake-research; };

      # Contributor E2E tooling can move ahead of the nixpkgs package while
      # remaining source- and dependency-hash pinned.
      devcontainerCli = import ./lib/devcontainer-cli.nix { inherit pkgs; };

      # patchelf 0.19.2 cannot safely rewrite the locked upstream wheel's ELF
      # layout. Invoke the unmodified binary through the Nix dynamic loader
      # with an explicit library path instead; do not change dependency pins.
      semgrep =
        (pkgs.semgrep.override {
          # Semgrep 1.180 imports FastMCP, removed from the default MCP 2.x.
          mcp = pkgs.python3Packages.mcp_1;
          semgrep-core = pkgs.semgrep-core.overrideAttrs {
            postFixup = ''
              mkdir -p "$out/libexec"
              mv "$out/bin/semgrep-core" "$out/libexec/semgrep-core"
              loader=$(cat ${pkgs.stdenv.cc}/nix-support/dynamic-linker)
              "$CC" -O2 -Wall -Wextra -Werror \
                -DLOADER="\"$loader\"" \
                -DCORE="\"$out/libexec/semgrep-core\"" \
                -DLIBRARIES="\"$out/bin/libs:${lib.makeLibraryPath [ pkgs.stdenv.cc.libc ]}\"" \
                ${./lib/semgrep-loader.c} -o "$out/bin/semgrep-core"
            '';
            doInstallCheck = true;
            installCheckPhase = ''
              runHook preInstallCheck
              "$out/bin/semgrep-core" -version
              runHook postInstallCheck
            '';
          };
        }).overrideAttrs
          (old: {
            doInstallCheck = true;
            postInstallCheck = (old.postInstallCheck or "") + ''
              export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
              export PATH="$out/bin:$PATH"
              "$out/bin/semgrep" --version
              # Leave installCheckPhase unset so pytestCheckHook retains its suite.
            '';
          });

      # Security tools use the locked nixpkgs revisions; rule updates are
      # separate, reviewable hash bumps instead of live registry downloads.
      securityTools = pkgs.symlinkJoin {
        name = "tentaflake-security-tools";
        paths = [
          pkgs.osv-scanner
          semgrep
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

        # Installer and optional profiles are exported explicitly;
        # none of them enlarge nixosModules.default.
        installer = import ./installer/iso.nix;
        observability = import ./modules/profiles/observability.nix;
        falco = import ./modules/profiles/falco.nix;
      };

      # ── Exported helpers ──
      lib.${system} = {
        inherit
          agentsFromData
          runtimeCatalog
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
        host-api = self.packages.${system}.tentaflake-host-api;
        host-api-evaluation = import ./tests/host-api-evaluation.nix { inherit pkgs; };
        devcontainer-cli = self.packages.${system}.devcontainer-cli;
        ci-vm-selection = import ./tests/ci-vm-selection.nix { inherit pkgs; };
        rust-package-sources = import ./tests/rust-package-sources.nix { inherit self pkgs; };
        image-pinning = import ./lib/pinnedImage-test.nix { inherit pkgs; };
        hardening-journald = import ./tests/hardening-journald.nix { inherit pkgs; };
        agent-adapters =
          let
            tests = import ./tests/agent-adapters.nix { inherit pkgs; };
          in
          pkgs.runCommand "tentaflake-agent-adapters"
            {
              nativeBuildInputs = [
                pkgs.python3
                (lib.getBin pkgs.nix)
                pkgs.bash
                pkgs.coreutils
                pkgs.findutils
                pkgs.gnugrep
                pkgs.util-linux
              ];
              quotaPrepare = pkgs.writeText "state-quota-prepare" tests.prepareScript;
              quotaOwner = pkgs.writeText "state-quota-owner" tests.ownerScript;
              src = lib.fileset.toSource {
                root = ./.;
                fileset = lib.fileset.unions [
                  ./adapters
                  ./lib/runtimeCatalog.nix
                  ./lib/agentIdentity.nix
                  ./lib/stoppedScaffold.nix
                  ./docs/agent-adapters.md
                  ./scripts/runtime-catalog-docs.py
                  ./scripts/state-quota-test.py
                ];
              };
            }
            ''
              # Even read-only Nix startup needs writable local profile metadata.
              export NIX_STATE_DIR="$TMPDIR/nix-state"
              python3 "$src/scripts/runtime-catalog-docs.py" --check --root "$src"
              python3 "$src/scripts/state-quota-test.py" "$quotaPrepare" "$quotaOwner"
              touch "$out"
            '';
        module-evaluation = import ./tests/module-eval.nix {
          nixpkgsPath = nixpkgs.outPath;
          researchFlake = inputs.tentaflake-research;
        };

        # VM integration test: boots the host and asserts the runtime path
        # (Rust CLI, status, and agent unit/user/state dir).
        vm-integration = pkgs.testers.runNixOSTest (
          import ./tests/integration.nix { inherit self agentsFromData; }
        );
        research-integration = import ./tests/research-integration.nix { inherit self pkgs; };
        worker-drain-wakeup = import ./tests/worker-drain-wakeup.nix { inherit self pkgs; };
        research-policy = import ./tests/research-policy.nix { inherit self pkgs; };
      };

      # ── tentaflake: Installed system, consumes agents.json ──
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
        tentaflake-host-api = pkgs.callPackage ./pkgs/tentaflake-host-api { };
        tentaflake-worker-image = pkgs.callPackage ./pkgs/tentaflake-worker/image.nix { };
        installer-iso = self.nixosConfigurations.installer-iso.config.system.build.isoImage;
      };
    };
}
