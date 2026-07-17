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
    "kernel.kexec_load_disabled" = 1;
    "kernel.sysrq" = 0;
    "net.core.bpf_jit_harden" = 2;
    "net.ipv4.tcp_rfc1337" = 1;
    "net.ipv4.icmp_echo_ignore_broadcasts" = 1;
    "net.ipv4.icmp_ignore_bogus_error_responses" = 1;
    # rp_filter=2 (loose) not 1 (strict): strict breaks Docker bridge networking
    # and multi-homed routing on this host.
    "net.ipv4.conf.all.rp_filter" = 2;
    "net.ipv4.conf.default.rp_filter" = 2;
    "net.ipv4.conf.all.log_martians" = 1;
    "net.ipv4.conf.default.log_martians" = 1;
    "net.ipv4.conf.all.arp_ignore" = 1;
    "net.ipv4.conf.default.arp_ignore" = 1;
    "net.ipv4.conf.all.arp_announce" = 2;
    "net.ipv4.conf.default.arp_announce" = 2;
  };

  boot.kernelParams = [
    "slab_nomerge"
    "init_on_alloc=1"
    "init_on_free=1"
    "pti=on"
    "vsyscall=none"
    "debugfs=off"
    "randomize_kstack_offset=on"
    "lsm=landlock,yama,apparmor,bpf"
  ];

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
