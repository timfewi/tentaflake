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
  };
}
