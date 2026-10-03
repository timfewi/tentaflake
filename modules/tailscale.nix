{
  config,
  lib,
  ...
}:
let
  cfg = config.tentaflake;
in
lib.mkIf cfg.tailscale.enable {
  services.tailscale = {
    enable = true;
    openFirewall = true;
    # Apply preferences independently of authKeyFile. extraUpFlags only run
    # during NixOS autoconnect and silently skip manually enrolled hosts.
    extraSetFlags = [
      "--advertise-tags=tag:agent-host"
      "--hostname=${cfg.hostName}"
      "--ssh"
    ];
  };
}
