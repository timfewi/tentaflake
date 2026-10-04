{
  config,
  lib,
  ...
}:
let
  cfg = config.tentaflake;
  preferences = [
    "--hostname=${cfg.hostName}"
    "--ssh"
  ];
in
lib.mkIf cfg.tailscale.enable {
  services.tailscale = {
    enable = true;
    openFirewall = true;
    # Apply supported preferences even without authKeyFile. Tags are up-only;
    # manual enrollment must supply them explicitly (see the management guide).
    extraSetFlags = preferences;
    extraUpFlags = [ "--advertise-tags=tag:agent-host" ] ++ preferences;
  };
}
