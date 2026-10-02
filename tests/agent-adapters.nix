{ pkgs }:
let
  inherit (pkgs) lib;
  builders = import ../lib { inherit pkgs lib; };
  eval =
    modules:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
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
          virtualisation.oci-containers.backend = "docker";
          tentaflake = {
            adminUser = "operator";
            security.profile = "balanced";
            boot.enable = false;
            locale.enable = false;
            networking.enable = false;
            nixSettings.enable = false;
            packages.enable = false;
            shell.enable = false;
          };
        }
      ]
      ++ modules;
    }).config;
  instance =
    adapter: name:
    builders.mkAgent {
      inherit adapter name;
      autoStart = false;
    };
  config = eval [
    (builders.mkHermesAgent {
      name = "legacy";
      autoStart = false;
      stateDir = "/var/lib/legacy-home";
      containerUid = 12000;
      containerGid = 12001;
      user = "legacy-host";
      group = "legacy-host";
      uid = 12002;
      gid = 12003;
      seedDir = ./fixtures/adapter-seed;
      gitIdentity = {
        name = "fixture";
        email = "fixture@example.invalid";
      };
    })
    (builders.mkZeroClawAgent {
      name = "legacy";
      autoStart = false;
      seedDir = ./fixtures/adapter-seed;
    })
    (instance "hermes" "zeroclaw-named")
    (instance "zeroclaw" "hermes-named")
    (instance "openclaw" "assistant")
  ];
  containers = config.virtualisation.oci-containers.containers;
  records = config.tentaflake.agentInstances;
  failures = c: map (a: a.message) (lib.filter (a: !a.assertion) c.assertions);
  denies =
    text: extra:
    lib.any (lib.hasInfix text) (
      failures (eval [
        (builders.mkAgent (
          {
            adapter = "hermes";
            name = "negative";
            autoStart = false;
          }
          // extra
        ))
      ])
    );
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
  inventory = import ../lib/agentInventory.nix { inherit lib; } {
    backend = "podman";
    containers = {
      "misleading-volume".volumes = [ "/not-state:/other:ro" ];
      "unmanaged".volumes = [ "/legacy-fallback:/data:rw" ];
    };
    instances."misleading-volume" = {
      adapter = "openclaw";
      name = "hermes-assistant";
      container = "misleading-volume";
      unit = "declared.service";
      stateDir = "/declared-state";
    };
  };
  json =
    value:
    builders.agentsFromData {
      file = builtins.toFile "adapter-input.json" (builtins.toJSON value);
      inherit (builders) mkAgent mkHermesAgent mkZeroClawAgent;
    };
  legacy = {
    hermes = [
      {
        name = "from-data";
        model = "fixture-model";
        provider = "fixture-provider";
        envFile = null;
        base_url = "http://fixture.invalid/v1";
      }
    ];
    zeroclaw = [
      {
        name = "from-data";
        model = "fixture-model";
        provider = "fixture-provider";
        envFile = null;
        hostPort = null;
        servePort = null;
      }
    ];
  };
  generic = {
    schemaVersion = 1;
    agents = [
      {
        adapter = "hermes";
        name = "generic";
        autoStart = false;
        settings.model.default = "fixture-model";
      }
    ];
  };
  dataConfig = eval (
    (json (legacy // generic)) ++ [ { tentaflake.security.profile = lib.mkForce "dev"; } ]
  );
  registryNames = builtins.attrNames (
    import ../adapters {
      pkgs = throw "inactive package evaluation";
      lib = throw "inactive library evaluation";
    }
  );
  contract = import ../lib/adapterContract.nix { inherit lib; };
  single = eval [ (instance "hermes" "only") ];
  forcedStart = eval [
    (instance "hermes" "forced")
    { virtualisation.oci-containers.containers.hermes-forced.autoStart = lib.mkForce true; }
  ];
  devRoot = eval [
    (builders.mkHermesAgent {
      name = "dev-root";
      autoStart = false;
      createUser = false;
      containerUid = 0;
      containerGid = 0;
      extraContainerConfig.user = "0:0";
    })
    { tentaflake.security.profile = lib.mkForce "dev"; }
  ];
in
assert devRoot.tentaflake.agentInstances.hermes-dev-root.uid == 0;
assert devRoot.virtualisation.oci-containers.containers.hermes-dev-root.user == "0:0";
assert failures devRoot == [ ];
assert lib.any (lib.hasInfix "requires an enabled broker") (failures forcedStart);
assert builtins.isFunction builders.mkHermesAgent && builtins.isFunction builders.mkZeroClawAgent;
assert
  builtins.functionArgs builders.mkHermesAgent
  == builtins.functionArgs builders.adapters.hermes.build;
assert
  builtins.functionArgs builders.mkZeroClawAgent
  == builtins.functionArgs builders.adapters.zeroclaw.build;
assert lib.all (unit: !(lib.hasInfix "zeroclaw" unit) && !(lib.hasInfix "openclaw" unit)) (
  builtins.attrNames single.systemd.services
);
assert rejects (json {
  schemaVersion = 1;
  agents = [
    {
      adapter = "hermes";
      name = "invalid";
      unknown = true;
    }
  ];
});
assert rejects (json {
  schemaVersion = 1;
  agents = [
    {
      adapter = "hermes";
      name = "invalid";
      autoStart = "yes";
    }
  ];
});
assert rejects (json {
  schemaVersion = 1;
  agents = [
    {
      adapter = "hermes";
      name = "invalid";
      settings.model.api_key = "fixture-only";
    }
  ];
});
assert
  registryNames == [
    "hermes"
    "openclaw"
    "zeroclaw"
  ];
assert rejects (contract "broken" { });
assert rejects (contract "hermes" (builders.adapters.hermes // { schemaVersion = 2; }));
assert
  builtins.attrNames records == [
    "hermes-legacy"
    "hermes-zeroclaw-named"
    "openclaw-assistant"
    "zeroclaw-hermes-named"
    "zeroclaw-legacy"
  ];
assert
  builtins.attrNames containers == [
    "hermes-legacy"
    "hermes-zeroclaw-named"
    "zeroclaw-hermes-named"
    "zeroclaw-legacy"
  ];
assert containers.hermes-legacy.user == "12000:12001";
assert records.hermes-legacy.stateDir == "/var/lib/legacy-home";
assert records.hermes-legacy.unit == "docker-hermes-legacy.service";
assert config.users.users.legacy-host.uid == 12002;
assert config.users.groups.legacy-host.gid == 12003;
assert lib.hasInfix "cp -rn" config.systemd.services.seed-hermes-legacy.script;
assert lib.hasInfix "/var/lib/zeroclaw-legacy/.zeroclaw/data/"
  config.systemd.services.seed-zeroclaw-legacy.script;
assert config.systemd.services.hermes-legacy-git-identity.script != "";
assert
  records.zeroclaw-legacy.uid == 65534
  && records.zeroclaw-legacy.workspace == "/var/lib/zeroclaw-legacy/data";
assert records.openclaw-assistant.runnable == false;
assert config.systemd.services.docker-openclaw-assistant.wantedBy == [ ];
assert lib.hasInfix "acceptance evidence" config.systemd.services.docker-openclaw-assistant.script;
assert lib.elem "--network=none" containers.hermes-zeroclaw-named.extraOptions;
assert failures config == [ ];
assert lib.hasInfix
  "openclaw\thermes-assistant\tmisleading-volume\tdeclared.service\t/declared-state"
  inventory;
assert lib.hasInfix "agent\tunmanaged\tunmanaged\tpodman-unmanaged.service\t/legacy-fallback"
  inventory;
assert dataConfig.tentaflake.agentInstances.hermes-generic.name == "generic";
assert dataConfig.virtualisation.oci-containers.containers.hermes-from-data.autoStart;
assert rejects (json {
  agents = [ ];
});
assert rejects (json {
  schemaVersion = 2;
});
assert rejects (json {
  extra = [ ];
});
assert rejects (json {
  hermes = [ { name = "incomplete"; } ];
});
assert rejects (json {
  schemaVersion = 1;
  agents = [
    {
      adapter = "unknown";
      name = "invalid";
    }
  ];
});
assert rejects (
  json (
    legacy
    // {
      schemaVersion = 1;
      agents = [
        {
          adapter = "hermes";
          name = "from-data";
        }
      ];
    }
  )
);
assert rejects
  (eval [
    (instance "hermes" "same")
    (instance "hermes" "same")
  ]).tentaflake.agentInstances;
assert rejects (
  builders.mkAgent {
    adapter = "unknown";
    name = "invalid";
  }
);
assert rejects (
  builders.mkAgent {
    adapter = "hermes";
    name = "invalid";
    unknown = true;
  }
);
assert rejects (
  builders.mkAgent {
    adapter = "hermes";
    name = "invalid";
    settings = [ ];
  }
);
assert rejects
  (eval [
    (builders.mkAgent {
      adapter = "openclaw";
      name = "invalid";
      autoStart = true;
    })
  ]).tentaflake.agentInstances;
assert denies "credential" { envFile = "/run/provider.env"; };
assert denies "secret-like settings" { settings.model.api_key = "fixture-only"; };
assert denies "publish container ports" { extraContainerConfig.ports = [ "8080:8080" ]; };
assert denies "sensitive bind mount" { extraVolumes = [ "/etc:/etc:rw" ]; };
assert denies "override a security invariant" {
  extraContainerConfig.extraOptions = [ "--privileged" ];
};
assert denies "requires an enabled broker" { autoStart = true; };
assert denies "requires an enabled broker" { extraContainerConfig.autoStart = true; };
assert rejects
  (eval [
    (builders.mkHermesAgent {
      name = "mutable";
      autoStart = false;
      image = "example.invalid/runtime:latest";
    })
  ]).virtualisation.oci-containers.containers.hermes-mutable.image;
true
