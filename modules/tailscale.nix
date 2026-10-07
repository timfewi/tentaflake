{
  config,
  lib,
  ...
}:
let
  cfg = config.tentaflake;
  preferences = [
    "--hostname=${cfg.hostName}"
    (if cfg.management.ssh.policy == "tailnet-policy" then "--ssh" else "--ssh=false")
  ];
in
lib.mkIf (cfg.management.enable && cfg.management.transport == "tailscale") {
  services.tailscale = {
    enable = true;
    openFirewall = true;
    # Apply supported preferences even without authKeyFile. Tags are up-only;
    # manual enrollment must supply them explicitly (see the management guide).
    extraSetFlags = preferences;
    extraUpFlags = [ "--advertise-tags=tag:agent-host" ] ++ preferences;
  };
}
