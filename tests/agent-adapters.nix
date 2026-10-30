{ pkgs }:
let
  inherit (pkgs) lib;
  builders = import ../lib { inherit pkgs lib; };
  mkAgent = import ../lib/mkAgent.nix { inherit pkgs lib; };
  adapters = import ../adapters { inherit pkgs lib; };
  internalParser = import ../lib/agentsFromData.nix { inherit pkgs lib; };
  exampleData = builtins.fromJSON (builtins.readFile ../agents.json.example);
  example = eval (builders.agentsFromData { file = ../agents.json.example; });
  exampleContainers = example.virtualisation.oci-containers.containers;
  security = import ../lib/containerSecurity.nix { inherit lib; };
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
    mkAgent {
      inherit adapter name;
      autoStart = false;
    };
  config = eval [
    (mkAgent {
      adapter = "hermes";
      name = "custom";
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
    (mkAgent {
      adapter = "zeroclaw";
      name = "zero-seeded";
      autoStart = false;
      seedDir = ./fixtures/adapter-seed;
    })
    (instance "hermes" "zeroclaw-named")
    (instance "zeroclaw" "hermes-named")
    (instance "openclaw" "assistant")
    (instance "pi" "pilot")
  ];
  containers = config.virtualisation.oci-containers.containers;
  records = config.tentaflake.agentInstances;
  failures = c: map (a: a.message) (lib.filter (a: !a.assertion) c.assertions);
  denies =
    text: extra:
    lib.any (lib.hasInfix text) (
      failures (eval [
        (mkAgent (
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
      "hermes-unmanaged".volumes = [ "/prefix-is-not-metadata:/data:rw" ];
      "zeroclaw-unmanaged".volumes = [ "/other-unmanaged:/data:rw" ];
    };
    instances."misleading-volume" = {
      adapter = "openclaw";
      name = "hermes-assistant";
      container = "misleading-volume";
      unit = "declared.service";
      stateDir = "/declared-state";
      runnable = false;
    };
  };
  # Real source files work even during read-only flake evaluation on a cold
  # store. toFile returns an unregistered path in that mode, masking validation.
  json =
    file:
    builders.agentsFromData {
      inherit file;
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
    image = "registry.example.invalid/fixture@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
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
    (mkAgent {
      adapter = "generic";
      name = "coding";
      inherit definition;
    })
  ];
  genericContainer = generic.virtualisation.oci-containers.containers.agent-coding;
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
    (mkAgent {
      adapter = "generic";
      name = "coding";
      inherit definition;
    })
    { virtualisation.oci-containers.containers.agent-coding.autoStart = lib.mkForce true; }
  ];
  genericPodman = eval [
    (mkAgent {
      adapter = "generic";
      name = "coding";
      inherit definition;
    })
    { virtualisation.oci-containers.backend = lib.mkForce "podman"; }
  ];
  single = eval [ (instance "hermes" "only") ];
  forcedStart = eval [
    (instance "hermes" "forced")
    { virtualisation.oci-containers.containers.agent-forced.autoStart = lib.mkForce true; }
  ];
  devRoot = eval [
    (mkAgent {
      adapter = "hermes";
      name = "dev-root";
      autoStart = false;
      createUser = false;
      containerUid = 0;
      containerGid = 0;
      extraContainerConfig.user = "0:0";
    })
    { tentaflake.security.profile = lib.mkForce "dev"; }
  ];
  defaults = eval (
    map (
      adapter:
      mkAgent (
        {
          inherit adapter;
          name = "default-${adapter}";
        }
        // lib.optionalAttrs ((builtins.functionArgs adapters.${adapter}.build) ? autoStart) {
          autoStart = false;
        }
        // lib.optionalAttrs (adapter == "generic") { inherit definition; }
      )
    ) registryNames
  );
  scaffoldsOnly = eval [
    (instance "openclaw" "refusal-openclaw")
    (instance "pi" "refusal-pi")
  ];
  hasParentGuard = service: lib.hasInfix "refusing unsafe agent parent" service.preStart;
in
assert !(builders ? mkAgent) && !(builders ? adapters);
assert builtins.attrNames (builtins.functionArgs builders.agentsFromData) == [ "file" ];
assert
  builtins.attrNames (builtins.functionArgs internalParser) == [
    "data"
    "file"
  ];
assert rejects (internalParser {
  file = ../agents.json.example;
  data = exampleData;
});
assert rejects (internalParser { });
assert
  builtins.length (internalParser {
    data = exampleData;
  }) == builtins.length exampleData.agents;
assert failures example == [ ];
assert builtins.length exampleData.agents == 4;
assert
  map (entry: entry.adapter) (lib.filter (entry: entry ? settings) exampleData.agents) == [
    "hermes"
    "zeroclaw"
  ];
assert lib.all (preset: !(preset ? build)) (lib.attrValues builders.runtimeCatalog.presets);
assert
  builtins.attrNames example.tentaflake.agentInstances
  == map (entry: "agent-${entry.name}") (lib.sort (a: b: a.name < b.name) exampleData.agents);
assert lib.all (
  entry:
  !entry.autoStart
  && !security.containsSensitiveValue (entry.settings or { })
  && example.tentaflake.agentInstances."agent-${entry.name}".adapter == entry.adapter
) exampleData.agents;
assert lib.all (container: !container.autoStart && container.environmentFiles == [ ]) (
  lib.attrValues exampleContainers
);
assert lib.all (
  entry:
  let
    container = "agent-${entry.name}";
    destination =
      if entry.adapter == "hermes" then "/state/config.yaml" else "/zeroclaw-data/.zeroclaw/config.toml";
    generated = adapters.${entry.adapter}.configuration.generate container entry.settings;
  in
  lib.elem "${generated}:${destination}:ro" exampleContainers.${container}.volumes
  && lib.elem "--pids-limit=512" exampleContainers.${container}.extraOptions
) (lib.filter (entry: entry ? settings) exampleData.agents);
assert !(exampleContainers ? agent-preview) && !(exampleContainers ? agent-scratch);
assert !example.tentaflake.agentInstances.agent-preview.runnable;
assert !example.tentaflake.agentInstances.agent-scratch.runnable;
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
assert genericJson.tentaflake.agentInstances.agent-from-json.adapter == "generic";
assert
  genericJson.virtualisation.oci-containers.containers.agent-from-json.cmd == [
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
    "/var/lib/tentaflake/agents/coding/state:/state:rw"
    "/var/lib/tentaflake/agents/coding/workspace:/workspace:rw"
  ];
assert lib.elem "d /var/lib/tentaflake/agents/coding 0700 root root -"
  generic.systemd.tmpfiles.rules;
assert lib.elem "d /var/lib/tentaflake/agents/coding/state 0700 10000 10000 -"
  generic.systemd.tmpfiles.rules;
assert
  generic.tentaflake.agentInstances.agent-coding.workspace
  == "/var/lib/tentaflake/agents/coding/workspace";
assert genericPodman.tentaflake.agentInstances.agent-coding.unit == "podman-agent-coding.service";
assert
  genericPodman.virtualisation.oci-containers.containers.agent-coding.preRunExtraOptions == [
    "--runtime"
    "runsc"
  ];
assert lib.any (lib.hasInfix "requires an enabled broker") (failures genericStarted);
assert lib.any (lib.hasInfix "requires its isolated Research relay") (failures genericStarted);
assert lib.all (bad: rejects (runtimeContract (definition // bad))) [
  { schemaVersion = 2; }
  { schemaVersion = 1.0; }
  { schemaVersion = "1"; }
  { schemaVersion = true; }
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
assert lib.all (name: (contract name adapters.${name}).identity.id == name) registryNames;
assert devRoot.tentaflake.agentInstances.agent-dev-root.uid == 0;
assert devRoot.virtualisation.oci-containers.containers.agent-dev-root.user == "0:0";
assert failures devRoot == [ ];
assert lib.any (lib.hasInfix "requires an enabled broker") (failures forcedStart);
assert builtins.isFunction mkAgent;
assert lib.all (
  adapter:
  let
    name = "default-${adapter}";
    record = defaults.tentaflake.agentInstances."agent-${name}";
  in
  record.adapter == adapter
  && record.name == name
  && record.container == "agent-${name}"
  && record.unit == "docker-agent-${name}.service"
  && record.stateDir == "/var/lib/tentaflake/agents/${name}"
  && record.workspace == "/var/lib/tentaflake/agents/${name}/${adapters.${adapter}.layout.workspace}"
  && record.uid == builders.runtimeCatalog.presets.${adapter}.ownership.uid
  && record.gid == builders.runtimeCatalog.presets.${adapter}.ownership.gid
) registryNames;
assert failures defaults == [ ];
assert hasParentGuard defaults.systemd.services.systemd-tmpfiles-setup;
assert hasParentGuard generic.systemd.services.systemd-tmpfiles-setup;
assert lib.all
  (
    adapter:
    let
      unit = defaults.systemd.services."docker-agent-default-${adapter}";
    in
    hasParentGuard unit
    && lib.elem "systemd-tmpfiles-setup.service" unit.requires
    && lib.elem "systemd-tmpfiles-setup.service" unit.after
  )
  [
    "generic"
    "hermes"
    "zeroclaw"
  ];
assert lib.all
  (
    name:
    let
      unit = config.systemd.services.${name};
    in
    lib.elem "systemd-tmpfiles-setup.service" unit.requires
    && lib.elem "systemd-tmpfiles-setup.service" unit.after
  )
  [
    "agent-custom-heal-uid"
    "agent-custom-seed"
    "agent-zero-seeded-seed"
  ];
assert
  !(scaffoldsOnly.systemd.services ? systemd-tmpfiles-setup)
  || !(hasParentGuard scaffoldsOnly.systemd.services.systemd-tmpfiles-setup);
assert lib.all
  (
    adapter:
    let
      unit = scaffoldsOnly.systemd.services."docker-agent-refusal-${adapter}";
    in
    !(hasParentGuard unit) && !(lib.elem "systemd-tmpfiles-setup.service" unit.requires)
  )
  [
    "openclaw"
    "pi"
  ];
assert containers.agent-custom.environment.HERMES_HOME == "/state";
assert lib.elem "/var/lib/legacy-home:/state:rw" containers.agent-custom.volumes;
assert lib.elem "/var/lib/tentaflake/agents/zeroclaw-named:/state:rw"
  containers.agent-zeroclaw-named.volumes;
assert lib.any (lib.hasSuffix ":/state/config.yaml:ro")
  dataConfig.virtualisation.oci-containers.containers.agent-generic.volumes;
assert lib.elem "/var/lib/tentaflake/agents/zero-seeded:/zeroclaw-data:rw"
  containers.agent-zero-seeded.volumes;
assert !(builders ? mkHermesAgent) && !(builders ? mkZeroClawAgent);
assert !(builtins.pathExists ../lib/mkHermesAgent.nix);
assert !(builtins.pathExists ../lib/mkZeroClawAgent.nix);
assert lib.all (
  unit:
  !(lib.hasInfix "zeroclaw" unit)
  && !(lib.hasInfix "openclaw" unit)
  && !(lib.any (backend: lib.hasPrefix "${backend}-pi-" unit) [
    "docker"
    "podman"
  ])
) (builtins.attrNames single.systemd.services);
assert lib.all (name: lib.hasInfix "agent\t${name}\t${name}\tpodman-${name}.service\t" inventory) [
  "hermes-unmanaged"
  "zeroclaw-unmanaged"
];
assert rejects (json ./fixtures/adapter-input/unknown-agent-field.json);
assert rejects (json ./fixtures/adapter-input/invalid-auto-start.json);
assert rejects (json ./fixtures/adapter-input/secret-setting.json);
assert
  registryNames == [
    "generic"
    "hermes"
    "openclaw"
    "pi"
    "zeroclaw"
  ];
assert rejects (contract "broken" { });
assert rejects (contract "hermes" (adapters.hermes // { schemaVersion = 2; }));
assert lib.all
  (
    directory:
    rejects (
      contract "hermes" (
        adapters.hermes
        // {
          layout = adapters.hermes.layout // {
            directories = [ directory ];
          };
        }
      )
    )
  )
  [
    "../outside"
    "/var/lib/other"
    "."
    "state//cache"
    1
  ];
assert
  builtins.attrNames records == [
    "agent-assistant"
    "agent-custom"
    "agent-hermes-named"
    "agent-pilot"
    "agent-zero-seeded"
    "agent-zeroclaw-named"
  ];
assert
  builtins.attrNames containers == [
    "agent-custom"
    "agent-hermes-named"
    "agent-zero-seeded"
    "agent-zeroclaw-named"
  ];
assert containers.agent-custom.user == "12000:12001";
assert records.agent-custom.stateDir == "/var/lib/legacy-home";
assert records.agent-custom.unit == "docker-agent-custom.service";
assert config.users.users.legacy-host.uid == 12002;
assert config.users.groups.legacy-host.gid == 12003;
assert lib.hasInfix "cp -rn" config.systemd.services.agent-custom-seed.script;
assert lib.hasInfix "/var/lib/tentaflake/agents/zero-seeded/.zeroclaw/data/"
  config.systemd.services.agent-zero-seeded-seed.script;
assert config.systemd.services.agent-custom-git-identity.script != "";
assert
  records.agent-zero-seeded.uid == 65534
  && records.agent-zero-seeded.workspace == "/var/lib/tentaflake/agents/zero-seeded/data";
assert lib.hasInfix "opt-in streaming" config.systemd.services.docker-agent-assistant.script;
assert
  !(lib.hasInfix "current LLM broker rejects" config.systemd.services.docker-agent-assistant.script);
assert import ./stopped-scaffold.nix { inherit pkgs builders config; };
assert lib.elem "--network=none" containers.agent-zeroclaw-named.extraOptions;
assert failures config == [ ];
assert lib.hasInfix
  "openclaw\thermes-assistant\tmisleading-volume\tdeclared.service\t/declared-state\tfalse"
  inventory;
assert lib.hasInfix "agent\tunmanaged\tunmanaged\tpodman-unmanaged.service\t/legacy-fallback\ttrue"
  inventory;
assert dataConfig.tentaflake.agentInstances.agent-generic.name == "generic";
assert dataConfig.virtualisation.oci-containers.containers.agent-from-data.autoStart;
assert rejects (json ./fixtures/adapter-input/missing-schema.json);
assert rejects (json ./fixtures/adapter-input/unsupported-schema.json);
assert rejects (json ./fixtures/adapter-input/unknown-root-field.json);
assert rejects (json ./fixtures/adapter-input/incomplete-legacy.json);
assert rejects (json ./fixtures/adapter-input/unknown-adapter.json);
assert rejects (json ./fixtures/adapter-input/duplicate-identity.json);
assert rejects (json ./fixtures/adapter-input/legacy-mixed.json);
assert rejects
  (eval [
    (instance "hermes" "same")
    (instance "zeroclaw" "same")
  ]).tentaflake.agentInstances;
assert rejects
  (eval [
    (instance "hermes" "same")
    (instance "hermes" "same")
  ]).tentaflake.agentInstances;
assert rejects (mkAgent {
  adapter = "unknown";
  name = "invalid";
});
assert rejects (mkAgent {
  adapter = "hermes";
  name = "invalid";
  unknown = true;
});
assert rejects (mkAgent {
  adapter = "hermes";
  name = "obsolete-option";
  gitAutoPush = null;
});
assert rejects (mkAgent {
  adapter = "hermes";
  name = "invalid";
  settings = [ ];
});
assert rejects
  (eval [
    (mkAgent {
      adapter = "openclaw";
      name = "invalid";
      autoStart = true;
    })
  ]).tentaflake.agentInstances;
assert rejects
  (eval [
    (mkAgent {
      adapter = "pi";
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
    (mkAgent {
      adapter = "hermes";
      name = "mutable";
      autoStart = false;
      image = "example.invalid/runtime:latest";
    })
  ]).virtualisation.oci-containers.containers.agent-mutable.image;
import ./agent-state-quota.nix { inherit pkgs eval; }
