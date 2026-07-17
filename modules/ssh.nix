{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.ssh;
in
lib.mkIf cfg.enable {
  # Key-only sshd for hosts that need direct (non-Tailscale) SSH access.
  # Admin keys come from tentaflake.adminAuthorizedKeys (see modules/users.nix).
  services.openssh = {
    enable = true;
    settings = {
      PasswordAuthentication = false;
      KbdInteractiveAuthentication = false;
      PermitRootLogin = "no";
      MaxAuthTries = 3;
    };
  };

  # Brute-force protection. No jail tuning here — that is deployment-specific
  # and belongs in forks.
  services.fail2ban.enable = true;

  # The default firewall is deny-all (modules/networking.nix), so sshd is
  # unreachable unless we open its port.
  networking.firewall.allowedTCPPorts = [ 22 ];
}
