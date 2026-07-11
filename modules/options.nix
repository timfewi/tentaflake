{ lib, ... }:
let
  inherit (import ../lib/constants.nix)
    hostName
    adminUser
    adminDescription
    adminShell
    defaultLocale
    consoleKeyMap
    stateVersion
    ;
in
{
  imports = [
    (lib.mkRenamedOptionModule
      [ "tentaflake" "shell" "hermesCli" "enable" ]
      [ "tentaflake" "shell" "tentaflakeCli" "enable" ]
    )
  ];

  options.tentaflake = {
    hostName = lib.mkOption {
      type = lib.types.str;
      default = hostName;
      description = "System hostname";
    };
    adminUser = lib.mkOption {
      type = lib.types.str;
      default = adminUser;
      description = "Primary admin username";
    };
    adminDescription = lib.mkOption {
      type = lib.types.str;
      default = adminDescription;
      description = "Description for the admin user";
    };
    adminShell = lib.mkOption {
      type = lib.types.str;
      default = adminShell;
      description = "Shell for the admin user";
    };
    timeZone = lib.mkOption {
      type = lib.types.str;
      default = "UTC";
      description = "System timezone";
    };
    defaultLocale = lib.mkOption {
      type = lib.types.str;
      default = defaultLocale;
      description = "Default system locale";
    };
    consoleKeyMap = lib.mkOption {
      type = lib.types.str;
      default = consoleKeyMap;
      description = "Console keymap";
    };
    stateVersion = lib.mkOption {
      type = lib.types.str;
      default = stateVersion;
      description = "NixOS state version";
    };

    # ── Deployment profile, container backend, and feature toggles ──

    profile = lib.mkOption {
      type = lib.types.enum [
        "installed"
        "installer"
        "live"
      ];
      default = "installed";
      description = "Deployment profile: installed=full system, installer=ISO installer, live=live agent ISO";
    };

    allowUnfree = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Allow unfree packages in nixpkgs";
    };

    adminAuthorizedKeys = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "SSH public keys for the admin user";
    };

    containerBackend = lib.mkOption {
      type = lib.types.enum [
        "docker"
        "podman"
      ];
      default = "docker";
      description = "OCI container backend for agent containers (\"docker\" or \"podman\")";
    };

    # Module enable toggles
    boot = {
      enable = lib.mkEnableOption "systemd-boot EFI bootloader config" // {
        default = true;
      };
    };

    hardening = {
      enable = lib.mkEnableOption "kernel hardening and sysctl settings" // {
        default = true;
      };
    };

    locale = {
      enable = lib.mkEnableOption "locale and timezone settings" // {
        default = true;
      };
    };

    networking = {
      enable = lib.mkEnableOption "networking, firewall, and NetworkManager" // {
        default = true;
      };
    };

    nixSettings = {
      enable = lib.mkEnableOption "Nix daemon settings (experimental-features, GC, optimise)" // {
        default = true;
      };
    };

    packages = {
      enable = lib.mkEnableOption "common system packages (curl, git)" // {
        default = true;
      };
    };

    users = {
      enable = lib.mkEnableOption "admin user creation" // {
        default = true;
      };
    };

    tailscale = {
      enable = lib.mkEnableOption "Tailscale VPN" // {
        default = true;
      };
    };

    # ── Interactive shell experience for SSH/console operators ──
    shell = {
      enable =
        lib.mkEnableOption "improved interactive shell (prompt, completion, MOTD, tentaflake CLI, tools)"
        // {
          default = true;
        };

      motd = {
        enable = lib.mkEnableOption "dynamic login banner (tentaflake-status) on SSH/console login" // {
          default = true;
        };
      };

      tools = {
        enable =
          lib.mkEnableOption "curated modern CLI tools (eza, bat, fd, ripgrep, fzf, htop, btop, …)"
          // {
            default = true;
          };
      };

      starship = {
        enable = lib.mkEnableOption "the starship prompt (falls back to a colored bash prompt if off)" // {
          default = true;
        };
      };

      zsh = {
        enable =
          lib.mkEnableOption "zsh as the interactive/login shell (Oh My Zsh, autosuggestions, syntax highlighting, fzf-tab)"
          // {
            default = false;
          };
      };

      zoxide = {
        enable = lib.mkEnableOption "zoxide smart directory jumping (cross-shell)" // {
          default = true;
        };
      };

      lazygit = {
        enable = lib.mkEnableOption "lazygit terminal Git UI (adds the 'lg' alias)" // {
          default = false;
        };
      };

      tmux = {
        enable = lib.mkEnableOption "tmux terminal multiplexer with a sensible system config" // {
          default = false;
        };
      };

      tentaflakeCli = {
        enable = lib.mkEnableOption "the 'tentaflake' multi-runtime agent-management CLI" // {
          default = true;
        };
      };
    };
  };
}
