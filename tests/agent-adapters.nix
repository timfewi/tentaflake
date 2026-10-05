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
  # Real source files work even during read-only flake evaluation on a cold
  # store. toFile returns an unregistered path in that mode, masking validation.
  json =
    file:
    builders.agentsFromData {
      inherit file;
      inherit (builders) mkAgent mkHermesAgent mkZeroClawAgent;
    };
  genericJson = eval (json ./fixtures/adapter-input/generic.json);
  dataConfig = eval (
    (json ./fixtures/adapter-input/mixed.json)
    ++ [ { tentaflake.security.profile = lib.mkForce "dev"; } ]
  );
  registryNames = builtins.attrNames (
    import ../adapters {
      pkgs = throw "inactive package evaluation";
      lib = throw "inactive library evaluation";
    }
  );
  contract = import ../lib/adapterContract.nix { inherit lib; };
  runtimeContract = import ../lib/runtimeContract.nix { inherit lib; };
  definition = {
    schemaVersion = 1;
    image = builders.constants.hermesImage;
    command = [
      "/bin/sh"
      "-c"
      "printf 'hello workspace' > result.txt"
    ];
    resources = {
      memory = "512m";
      memorySwap = "512m";
      cpus = "0.5";
      pidsLimit = 64;
    };
  };
  generic = eval [
    (builders.mkAgent {
      adapter = "generic";
      name = "coding";
      inherit definition;
    })
  ];
  genericContainer = generic.virtualisation.oci-containers.containers.generic-coding;
  nestedSources =
    access:
    (import ../lib/containerSecurity.nix { inherit lib; }).apply {
      profile = "balanced";
      backend = "docker";
      name = "nested-source-fixture";
      owner = "10000:10000";
      pidsLimit = 64;
      resources = generic.tentaflake.security.resources;
      baseConfig = {
        inherit (definition) image;
        autoStart = false;
        volumes = [
          "/var/lib/fixture:/state:rw"
          "/var/lib/fixture/workspace:/workspace:${access}"
        ];
      };
      allowedWritableSources = [
        "/var/lib/fixture"
        "/var/lib/fixture/workspace"
      ];
      allowedWritableDestinations = [
        "/state"
        "/workspace"
      ];
      approvedReadOnlySources = [ "/var/lib/fixture/workspace" ];
      approvedReadOnlyDestinations = [ "/workspace" ];
    };
  genericStarted = eval [
    (builders.mkAgent {
      adapter = "generic";
      name = "coding";
      inherit definition;
    })
    { virtualisation.oci-containers.containers.generic-coding.autoStart = lib.mkForce true; }
  ];
  genericPodman = eval [
    (builders.mkAgent {
      adapter = "generic";
      name = "coding";
      inherit definition;
    })
    { virtualisation.oci-containers.backend = lib.mkForce "podman"; }
  ];
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
assert lib.all
  (
    access:
    lib.any (
      item: !item.assertion && lib.hasInfix "writable bind source" item.message
    ) (nestedSources access).assertions
  )
  [
    "rw"
    "ro"
  ];
assert failures genericJson == [ ];
assert genericJson.tentaflake.agentInstances.generic-from-json.adapter == "generic";
assert
  genericJson.virtualisation.oci-containers.containers.generic-from-json.cmd == [
    "fixture"
    "--workspace"
    "/workspace"
  ];
assert failures generic == [ ];
assert !genericContainer.autoStart;
assert genericContainer.cmd == definition.command;
assert genericContainer.user == "10000:10000";
assert lib.elem "--network=none" genericContainer.extraOptions;
assert lib.elem "--runtime=runsc" genericContainer.extraOptions;
assert lib.elem "--memory=512m" genericContainer.extraOptions;
assert lib.elem "--pids-limit=64" genericContainer.extraOptions;
assert
  genericContainer.volumes == [
    "/var/lib/generic-coding/state:/state:rw"
    "/var/lib/generic-coding/workspace:/workspace:rw"
  ];
assert lib.elem "d /var/lib/generic-coding 0700 root root -" generic.systemd.tmpfiles.rules;
assert lib.elem "d /var/lib/generic-coding/state 0700 10000 10000 -" generic.systemd.tmpfiles.rules;
assert
  generic.tentaflake.agentInstances.generic-coding.workspace == "/var/lib/generic-coding/workspace";
assert
  genericPodman.tentaflake.agentInstances.generic-coding.unit == "podman-generic-coding.service";
assert
  genericPodman.virtualisation.oci-containers.containers.generic-coding.preRunExtraOptions == [
    "--runtime"
    "runsc"
  ];
assert lib.any (lib.hasInfix "requires an enabled broker") (failures genericStarted);
assert lib.any (lib.hasInfix "requires its isolated Research relay") (failures genericStarted);
assert lib.all (bad: rejects (runtimeContract (definition // bad))) [
  { schemaVersion = 2; }
  { image = "example.invalid/agent:latest"; }
  { image = "image;echo unsafe@sha256:${lib.concatStrings (lib.replicate 64 "a")}"; }
  { command = "sh -c echo unsafe"; }
  { command = [ ]; }
  {
    ownership = {
      uid = 0;
      gid = 10000;
    };
  }
  {
    ownership = {
      uid = 10000;
      gid = 10000;
      privileged = true;
    };
  }
  { workspace = "/workspace/../etc"; }
  { volumes = [ "/home:/host:rw" ]; }
  {
    environment = {
      API_KEY = "fixture";
    };
  }
  { resources.memory = "0"; }
  { resources.cpus = "0.0"; }
  { resources.pidsLimit = null; }
  { resources.devices = [ "/dev/kvm" ]; }
  { lifecycle = "unmanaged"; }
  { capabilities = [ "research" ]; }
  {
    capabilities = [
      "shell"
      "shell"
    ];
  }
];
assert lib.all (name: (contract name builders.adapters.${name}).identity.id == name) registryNames;
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
assert rejects (json ./fixtures/adapter-input/unknown-agent-field.json);
assert rejects (json ./fixtures/adapter-input/invalid-auto-start.json);
assert rejects (json ./fixtures/adapter-input/secret-setting.json);
assert
  registryNames == [
    "generic"
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
assert lib.hasInfix "opt-in streaming" config.systemd.services.docker-openclaw-assistant.script;
assert
  !(lib.hasInfix "current LLM broker rejects" config.systemd.services.docker-openclaw-assistant.script);
assert lib.elem "--network=none" containers.hermes-zeroclaw-named.extraOptions;
assert failures config == [ ];
assert lib.hasInfix
  "openclaw\thermes-assistant\tmisleading-volume\tdeclared.service\t/declared-state"
  inventory;
assert lib.hasInfix "agent\tunmanaged\tunmanaged\tpodman-unmanaged.service\t/legacy-fallback"
  inventory;
assert dataConfig.tentaflake.agentInstances.hermes-generic.name == "generic";
assert dataConfig.virtualisation.oci-containers.containers.hermes-from-data.autoStart;
assert rejects (json ./fixtures/adapter-input/missing-schema.json);
assert rejects (json ./fixtures/adapter-input/unsupported-schema.json);
assert rejects (json ./fixtures/adapter-input/unknown-root-field.json);
assert rejects (json ./fixtures/adapter-input/incomplete-legacy.json);
assert rejects (json ./fixtures/adapter-input/unknown-adapter.json);
assert rejects (json ./fixtures/adapter-input/duplicate-identity.json);
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
