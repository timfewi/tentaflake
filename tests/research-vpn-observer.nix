{ self, pkgs }:
let
  inherit (pkgs) lib;
  # Inspect only observer/lease unit settings: no application or parser closure
  # realization is needed for these option and authority regressions.
  evaluate =
    extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        self.nixosModules.default
        {
          system.stateVersion = "26.05";
          fileSystems."/" = {
            device = "/dev/disk/by-label/nixos";
            fsType = "btrfs";
          };
          boot.loader.grub.devices = [ "nodev" ];
          users.users.operator.uid = 1000;
          tentaflake = {
            adminUser = "operator";
            boot.enable = false;
            locale.enable = false;
            networking.enable = false;
            nixSettings.enable = false;
            packages.enable = false;
            shell.enable = false;
          };
          services.secureResearch = {
            enable = true;
            serviceUid = 4201;
            egressUid = 4202;
            clients.operator = 1000;
            vpnInterface = "fixture-vpn";
            resolvers = [ "9.9.9.9" ];
          };
        }
        extra
      ];
    }).config;
  failures =
    config: map (entry: entry.message) (lib.filter (entry: !entry.assertion) config.assertions);
  denies = fragment: config: lib.any (message: lib.hasInfix fragment message) (failures config);
  valid = evaluate { };
  observer =
    extra:
    evaluate {
      services.secureResearch = {
        resolvers = lib.mkForce [
          "9.9.9.9"
          "2620:fe::fe"
        ];
        vpnObserver = {
          enable = true;
          firewallMarker = "/run/research-vpn/firewall.ready";
          egressPathEvidence = true;
          drainMarker = "/run/research-vpn/drain";
        }
        // extra;
      };
    };
  peerKey = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
  wireguard = observer {
    linkKind = "wireguard";
    handshakeWithinSeconds = 190;
    peerPublicKeys = [ peerKey ];
  };
  tun = observer { linkKind = "tun"; };
  disabledObserver = evaluate {
    services.secureResearch.vpnObserver.egressPathEvidence = true;
  };
  wrongPeerKind = observer {
    linkKind = "tun";
    peerPublicKeys = [ peerKey ];
  };
  wrongHandshakeKind = observer {
    linkKind = "tun";
    handshakeWithinSeconds = 190;
  };
  tooManyPeers = observer {
    linkKind = "wireguard";
    peerPublicKeys = lib.replicate 5 peerKey;
  };
  reusedMarker = observer {
    drainMarker = "/run/research-vpn/firewall.ready";
  };
  observerService = wireguard.systemd.services.agent-research-vpn-observer.serviceConfig;
  tunService = tun.systemd.services.agent-research-vpn-observer.serviceConfig;
in
assert lib.assertMsg (failures valid == [ ]) (builtins.toJSON (failures valid));
assert !valid.services.secureResearch.vpnObserver.enable;
assert valid.services.secureResearch.vpnObserver.linkKind == null;
assert !valid.services.secureResearch.vpnObserver.egressPathEvidence;
assert valid.services.secureResearch.vpnObserver.handshakeWithinSeconds == null;
assert valid.services.secureResearch.vpnObserver.peerPublicKeys == [ ];
assert valid.services.secureResearch.vpnObserver.drainMarker == null;
assert !(builtins.hasAttr "agent-research-vpn-observer" valid.systemd.services);
assert lib.assertMsg (failures wireguard == [ ]) (builtins.toJSON (failures wireguard));
assert lib.assertMsg (failures tun == [ ]) (builtins.toJSON (failures tun));
assert lib.all (flag: lib.hasInfix flag observerService.ExecStart) [
  " --link-kind wireguard"
  " --egress-uid 4202"
  " --dns-resolver 9.9.9.9"
  " --dns-resolver 2620:fe::fe"
  " --handshake-within 190"
  " --peer-public-key ${peerKey}"
  " --drain-marker /run/research-vpn/drain"
];
assert observerService.CapabilityBoundingSet == [ "CAP_NET_ADMIN" ];
assert observerService.User == "root";
assert
  observerService.RestrictAddressFamilies == [
    "AF_UNIX"
    "AF_NETLINK"
  ];
assert observerService.ReadWritePaths == [ "/run/research-vpn" ];
assert !(builtins.hasAttr "LoadCredential" observerService);
assert tunService.CapabilityBoundingSet == "";
assert lib.hasInfix " --link-kind tun" tunService.ExecStart;
assert !(lib.hasInfix "--handshake-within" tunService.ExecStart);
assert !(lib.hasInfix "--peer-public-key" tunService.ExecStart);
assert denies "apply only with vpnObserver.enable" disabledObserver;
assert denies "require vpnObserver.linkKind" wrongPeerKind;
assert denies "require vpnObserver.linkKind" wrongHandshakeKind;
assert denies "allows at most four keys" tooManyPeers;
assert denies "must differ" reusedMarker;
true
