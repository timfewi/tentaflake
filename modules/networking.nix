{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake;
  egress = cfg.networking.egress;
  portList = ports: lib.concatMapStringsSep ", " toString ports;
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

    # Opt-in egress allowlist. Agent containers run with --network=host
    # (lib/mkHermesAgent.nix), so these host OUTPUT rules cover them too.
    # Distinct table name so it never clashes with the firewall's own tables.
    # Order matters: loopback + established/related must be accepted first.
    nftables.tables.tentaflake-egress = lib.mkIf egress.enable {
      family = "inet";
      content = ''
        chain output {
          type filter hook output priority 0; policy drop;
          oifname "lo" accept
          ct state established,related accept
          # ICMPv6 is not conntrack-tracked; without this, neighbor discovery
          # is dropped and ALL IPv6 traffic breaks (allowlisted ports included).
          # ICMPv4 kept too for ping/PMTU diagnostics.
          meta l4proto { icmp, ipv6-icmp } accept
          ${lib.optionalString (egress.allowedTCPPorts != [ ]) ''
            tcp dport { ${portList egress.allowedTCPPorts} } accept
          ''}
          ${lib.optionalString (egress.allowedUDPPorts != [ ]) ''
            udp dport { ${portList egress.allowedUDPPorts} } accept
          ''}
          counter drop
        }
      '';
    };
  };
}
