{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake;
in
lib.mkIf cfg.tailscale.enable {
  services.tailscale = {
    enable = true;
    openFirewall = true;
    extraUpFlags = [
      "--advertise-tags=tag:agent"
      "--hostname=${cfg.hostName}"
      "--ssh"
    ];
  };
}
