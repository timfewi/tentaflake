{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake;
in
lib.mkIf cfg.networking.enable {
  networking = {
    hostName = cfg.hostName;
    networkmanager.enable = true;
    nftables.enable = true;
    firewall = {
      enable = true;
      allowPing = false;
      allowedTCPPorts = [ ];
      allowedUDPPorts = [ ];
      logRefusedConnections = true;
    };
  };
}
