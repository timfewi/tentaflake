{ pkgs }:
let
  inherit (pkgs) lib;
  zero = lib.genAttrs [
    "memoryBytes"
    "memorySwapBytes"
    "nanoCpus"
    "pids"
    "workspaceBytes"
    "stateBytes"
  ] (_: 0);
  base = import (pkgs.path + "/nixos/lib/eval-config.nix") {
    system = pkgs.stdenv.hostPlatform.system;
    specialArgs.researchFlake = null;
    modules = [
      ../modules/default.nix
      {
        system.stateVersion = "26.05";
        fileSystems."/" = {
          device = "/dev/disk/by-label/nixos";
          fsType = "btrfs";
        };
        boot.loader.grub.devices = [ "nodev" ];
        users.users.operator.uid = 1000;
        virtualisation.oci-containers.backend = "docker";
        tentaflake = {
          hostName = "api-fixture";
          adminUser = "operator";
          boot.enable = false;
          locale.enable = false;
          networking.enable = false;
          nixSettings.enable = false;
          packages.enable = false;
          shell.enable = false;
          security.admission = {
            authorityEpoch = "api-module-fixture";
            capacity = zero;
            hostReserve = zero;
            researchReserve = zero;
            maxActive = 1;
            physicalSpaceHeadroomBytes = 0;
          };
        };
      }
    ];
  };
  configured = base.extendModules {
    modules = [
      {
        tentaflake.api = {
          enable = true;
          host = "host-api-fixture";
          origin = "https://host.example.invalid";
          principal = {
            id = "operator-fixture";
            name = "Fixture operator";
          };
          operatorTokenFile = "/run/credentials/fixture-operator-token";
          cliBearerFile = "/run/credentials/fixture-cli-bearer";
        };
      }
    ];
  };
  configuration = configured.config.tentaflake.api.configuration;
  document = builtins.fromJSON (builtins.unsafeDiscardStringContext configuration.text);
  plan = builtins.fromJSON (
    builtins.unsafeDiscardStringContext configured.config.tentaflake.admissionManifest.text
  );
  frontend = configured.config.systemd.services.tentaflake-api;
  service = frontend.serviceConfig;
  owner = configured.config.systemd.services.tentaflake-api-owner;
  socket = configured.config.systemd.sockets.tentaflake-api-owner;
  catalogGrant = {
    id = "fixture-catalog-read";
    action = "catalog.read";
    target = {
      kind = "catalog";
      id = "fixture-catalog";
    };
    expiresAt = null;
  };
  catalogEnabled = configured.extendModules {
    modules = [ { tentaflake.api.catalogReadGrant = catalogGrant; } ];
  };
  catalogConfiguration = catalogEnabled.config.tentaflake.api.configuration;
  catalogDocument = builtins.fromJSON (builtins.unsafeDiscardStringContext catalogConfiguration.text);
  catalogPlan = builtins.fromJSON (
    builtins.unsafeDiscardStringContext catalogEnabled.config.tentaflake.admissionManifest.text
  );
  refusesCatalogGrant =
    grant:
    let
      fixture = configured.extendModules {
        modules = [ { tentaflake.api.catalogReadGrant = lib.mkForce grant; } ];
      };
    in
    !(builtins.tryEval fixture.config.tentaflake.api.configuration.text).success;
  refuses =
    fragment: settings:
    let
      fixture = configured.extendModules { modules = [ settings ]; };
    in
    lib.any (entry: !entry.assertion && lib.hasInfix fragment entry.message) fixture.config.assertions;
in
assert lib.all (entry: entry.assertion) configured.config.assertions;
assert base.config.tentaflake.api.configuration == null;
assert !(base.config.systemd.services ? tentaflake-api);
assert !(base.config.systemd.services ? tentaflake-api-owner);
assert !(base.config.systemd.sockets ? tentaflake-api-owner);
assert !(base.config.users.users ? tentaflake-api);
assert
  (builtins.fromJSON (
    builtins.unsafeDiscardStringContext base.config.tentaflake.admissionManifest.text
  )).apiConfig == null;
assert plan.apiConfig == toString configuration;
assert configured.config.tentaflake.api.catalogReadGrant == null;
assert lib.all (entry: entry.assertion) catalogEnabled.config.assertions;
assert catalogDocument == document // { catalogReadGrant = catalogGrant; };
assert toString catalogConfiguration != toString configuration;
assert catalogPlan.apiConfig == toString catalogConfiguration;
assert catalogPlan.policy.revision != plan.policy.revision;
assert lib.all refusesCatalogGrant [
  (catalogGrant // { id = "invalid:id"; })
  (catalogGrant // { action = "catalog.import"; })
  (
    catalogGrant
    // {
      target = {
        kind = "host";
        id = "fixture-catalog";
      };
    }
  )
  (
    catalogGrant
    // {
      target = {
        kind = "catalog";
        id = "invalid:id";
      };
    }
  )
  (catalogGrant // { expiresAt = 1; })
  (catalogGrant // { principal = "other-owner"; })
];
assert
  document == {
    schemaVersion = 1;
    host = "host-api-fixture";
    origin = "https://host.example.invalid";
    listen = "127.0.0.1:18181";
    principal = {
      id = "operator-fixture";
      name = "Fixture operator";
    };
    operatorLogin = true;
    cliBearer = true;
    catalogReadGrant = null;
  };
assert
  service.ExecStart == lib.escapeShellArgs [
    plan.tools.cli
    "api"
    "serve"
    (toString configuration)
  ];
assert !(service ? LoadCredential);
assert !(service ? ReadWritePaths);
assert !(service ? StateDirectory);
assert service.User == "tentaflake-api" && service.Group == "tentaflake-api";
assert configured.config.users.users.tentaflake-api.isSystemUser;
assert configured.config.users.users.tentaflake-api.group == "tentaflake-api";
assert frontend.requires == [ "tentaflake-api-owner.socket" ];
assert lib.elem "tentaflake-api-owner.socket" frontend.after;
assert !(lib.elem "tentaflake-api-owner.service" frontend.after);
assert socket.wantedBy == [ ];
assert socket.bindsTo == [ "tentaflake-api.service" ];
assert !(lib.elem "tentaflake-api.service" socket.after);
assert socket.partOf == [ "tentaflake-api.service" ];
assert socket.listenStreams == [ "/run/tentaflake-api-owner.sock" ];
assert socket.socketConfig.SocketUser == "root";
assert socket.socketConfig.SocketGroup == "tentaflake-api";
assert socket.socketConfig.SocketMode == "0660";
assert socket.socketConfig.RemoveOnStop;
assert owner.wantedBy == [ ];
assert owner.bindsTo == [ "tentaflake-api.service" ];
assert owner.partOf == [ "tentaflake-api.service" ];
assert owner.after == [ "tentaflake-api.service" ];
assert
  owner.serviceConfig.ExecStart == lib.escapeShellArgs [
    plan.tools.cli
    "api"
    "owner"
    (toString configuration)
    (toString configured.config.tentaflake.admissionManifest)
  ];
assert
  owner.serviceConfig.LoadCredential == [
    "operator-token:/run/credentials/fixture-operator-token"
    "cli-bearer:/run/credentials/fixture-cli-bearer"
  ];
assert owner.serviceConfig.User == "root" && owner.serviceConfig.Group == "root";
assert owner.serviceConfig.NoNewPrivileges;
assert owner.serviceConfig.CapabilityBoundingSet == "CAP_SYS_PTRACE";
assert owner.serviceConfig.AmbientCapabilities == "";
assert owner.serviceConfig.RestrictAddressFamilies == [ "AF_UNIX" ];
assert lib.all (option: !(builtins.hasAttr option owner.serviceConfig)) [
  "ProtectSystem"
  "ProtectHome"
  "PrivateTmp"
  "PrivateMounts"
  "ReadWritePaths"
  "StateDirectory"
];
assert service.ProtectSystem == "strict";
assert service.ProtectHome;
assert service.PrivateTmp;
assert service.NoNewPrivileges;
assert service.CapabilityBoundingSet == "";
assert service.AmbientCapabilities == "";
assert
  service.RestrictAddressFamilies == [
    "AF_INET"
    "AF_INET6"
    "AF_UNIX"
  ];
assert
  service.InaccessiblePaths == [
    "-/var/lib/tentaflake-admission"
    "-/var/lib/tentaflake-admission.lock"
    "-/run/credentials/tentaflake-api-owner.service"
  ];
assert refuses "host admission policy" { tentaflake.security.admission = lib.mkForce null; };
assert refuses "at least one" {
  tentaflake.api.operatorTokenFile = lib.mkForce null;
  tentaflake.api.cliBearerFile = lib.mkForce null;
};
assert refuses "distinct sources" {
  tentaflake.api.cliBearerFile = lib.mkForce "/run/credentials/fixture-operator-token";
};
assert lib.all (listen: refuses "loopback address" { tentaflake.api.listen = lib.mkForce listen; })
  [
    "0.0.0.0:18181"
    "[::]:18181"
    "127.0.0.1:0"
    "127.0.0.1:65536"
    "127.999.0.1:18181"
  ];
assert lib.all
  (
    listen:
    let
      fixture = configured.extendModules {
        modules = [ { tentaflake.api.listen = lib.mkForce listen; } ];
      };
    in
    lib.all (entry: entry.assertion) fixture.config.assertions
  )
  [
    "[::1]:18181"
    "127.2.3.4:18181"
  ];
assert lib.all
  (
    option:
    let
      fixture = configured.extendModules {
        modules = [ { tentaflake.api.${option} = lib.mkForce null; } ];
      };
      value = builtins.fromJSON (
        builtins.unsafeDiscardStringContext fixture.config.tentaflake.api.configuration.text
      );
    in
    lib.all (entry: entry.assertion) fixture.config.assertions
    && value.operatorLogin == (option != "operatorTokenFile")
    && value.cliBearer == (option != "cliBearerFile")
  )
  [
    "operatorTokenFile"
    "cliBearerFile"
  ];
assert lib.all (path: !configured.options.tentaflake.api.operatorTokenFile.type.check path) [
  ./api-policy.nix
  "/nix/store/example-secret"
  "/run/../secret"
  "relative-secret"
  "/run/secret\n"
];
assert !configured.options.tentaflake.api.host.type.check "invalid:host";
assert
  !configured.options.tentaflake.api.principal.id.type.check (
    lib.concatStrings (lib.replicate 97 "a")
  );
assert !configured.options.tentaflake.api.origin.type.check "https://host.example.invalid/";
assert !configured.options.tentaflake.api.origin.type.check "http://host.example.invalid";
true
