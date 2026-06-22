{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.hardening;
in
lib.mkIf cfg.enable {
  boot.kernel.sysctl = {
    "kernel.kptr_restrict" = 2;
    "kernel.dmesg_restrict" = 1;
    "kernel.unprivileged_bpf_disabled" = 1;
    "kernel.perf_event_paranoid" = 3;
    "vm.unprivileged_userfaultfd" = 0;
    "fs.protected_hardlinks" = 1;
    "fs.protected_symlinks" = 1;
    "fs.protected_fifos" = 2;
    "fs.protected_regular" = 2;
    "net.ipv4.tcp_syncookies" = 1;
    "net.ipv4.conf.all.accept_redirects" = 0;
    "net.ipv4.conf.default.accept_redirects" = 0;
    "net.ipv6.conf.all.accept_redirects" = 0;
    "net.ipv4.conf.all.accept_source_route" = 0;
    "net.ipv6.conf.all.accept_source_route" = 0;
    "user.max_user_namespaces" = 1000;
  };

  security = {
    sudo.wheelNeedsPassword = true;
    apparmor.enable = true;
  };

  services.journald.extraConfig = ''
    RateLimitIntervalSec=5s
    RateLimitBurst=1000
    Compress=yes
    SystemMaxUse=500M
  '';
}
