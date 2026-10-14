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
      nofile = 128;
      tmpfsSize = "32m";
      runTmpfsSize = "16m";
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
  genericEval =
    changes: extra:
    eval [
      (builders.mkAgent {
        adapter = "generic";
        name = "coding";
        definition = definition // changes;
      })
      extra
    ];
  genericQuota = {
    enable = true;
    workspace = "/var/lib/generic-coding/workspace";
    ownerUid = 10000;
    ownerGid = 10000;
    sizeMiB = 128;
    state = {
      path = "/var/lib/generic-coding/state";
      sizeMiB = 128;
    };
  };
  genericStorage.tentaflake.workspaceQuota.agents.generic-coding = genericQuota;
  # Only the source projection is evaluated here. Runtime descriptor validation
  # must inspect the actual archive before loading or starting an image.
  descriptorLocator = pkgs.writeText "fixture-local-image.json" "{}";
  localImage = {
    kind = "local-artifact";
    descriptor = descriptorLocator;
  };
  localDefinition = runtimeContract (definition // { image = localImage; });
  emptyReserve = lib.genAttrs [
    "memoryBytes"
    "memorySwapBytes"
    "nanoCpus"
    "pids"
    "workspaceBytes"
    "stateBytes"
  ] (_: 0);
  admissionPolicy = {
    authorityEpoch = "image-source-fixture";
    physicalSpaceHeadroomBytes = 67108864;
    capacity = {
      memoryBytes = 2147483648;
      memorySwapBytes = 2147483648;
      nanoCpus = 2000000000;
      pids = 512;
      workspaceBytes = 4294967296;
      stateBytes = 4294967296;
    };
    hostReserve = emptyReserve;
    researchReserve = emptyReserve;
    maxActive = 2;
  };
  localPolicy = lib.recursiveUpdate genericStorage {
    tentaflake.security.admission = admissionPolicy;
  };
  localGeneric = genericEval { image = localImage; } localPolicy;
  localContainer = localGeneric.virtualisation.oci-containers.containers.generic-coding;
  localSupervisor = localGeneric.systemd.services.docker-generic-coding;
  localManifest = builtins.fromJSON (
    builtins.unsafeDiscardStringContext localGeneric.tentaflake.admissionManifest.text
  );
  genericOffline = genericEval { lifecycle = "service"; } genericStorage;
  genericMissing = genericEval {
    capabilities = [
      "model"
      "research"
      "worker"
    ];
  } { };
  genericMissingStarted = genericEval {
    lifecycle = "service";
    capabilities = [
      "model"
      "research"
      "worker"
    ];
  } { };
  genericWorker = {
    enable = true;
    workspace = "/var/lib/generic-coding/workspace";
    containerUid = 10000;
    containerGid = 10000;
  };
  genericWorkerPolicy = lib.recursiveUpdate genericStorage {
    tentaflake.worker.agents.generic-coding = genericWorker;
  };
  genericWithWorker = genericEval {
    lifecycle = "service";
    capabilities = [ "worker" ];
  } genericWorkerPolicy;
  genericModelPolicy = lib.recursiveUpdate genericStorage {
    tentaflake.networking.enable = lib.mkForce true;
    tentaflake.broker.agents.generic-coding = {
      enable = true;
      networkName = "tf-generic-coding";
      subnet = "10.203.42.0/30";
      gateway = "10.203.42.1";
      llm = {
        enable = true;
        providerCredentialFile = "/run/credentials/fixture-provider";
        upstreamBaseUrl = "https://api.example.invalid/v1/";
        allowedModels = [
          {
            name = "fixture-model";
            inputMicrousdPerMillion = 1;
            outputMicrousdPerMillion = 1;
          }
        ];
      };
    };
  };
  genericWithModel = genericEval {
    lifecycle = "service";
    capabilities = [ "model" ];
  } genericModelPolicy;
  genericModelContainer = genericWithModel.virtualisation.oci-containers.containers.generic-coding;
  exceedsHost = {
    memory = "3g";
    memorySwap = "3g";
    cpus = "2.00001";
    pidsLimit = 513;
    nofile = 4097;
    tmpfsSize = "257m";
    runTmpfsSize = "65m";
  };
  excessiveResources = eval (
    lib.mapAttrsToList (
      key: value:
      builders.mkAgent {
        adapter = "generic";
        name = "ceiling-${lib.toLower key}";
        definition = definition // {
          resources =
            definition.resources
            // {
              ${key} = value;
            }
            // lib.optionalAttrs (key == "memory") { memorySwap = value; };
        };
      }
    ) exceedsHost
  );
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
assert import ./runtime-resources.nix { inherit lib; };
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
assert genericContainer.workdir == "/workspace";
assert !(lib.any (lib.hasPrefix "--workdir") genericContainer.extraOptions);
assert genericContainer.user == "10000:10000";
assert lib.elem "--network=none" genericContainer.extraOptions;
assert lib.elem "--runtime=runsc" genericContainer.extraOptions;
assert lib.elem "--memory=512m" genericContainer.extraOptions;
assert lib.elem "--pids-limit=64" genericContainer.extraOptions;
assert
  let
    line = lib.findFirst (lib.hasPrefix "agent\tgeneric-coding\t") "" (
      lib.splitString "\n" generic.environment.etc."tentaflake/security.tsv".text
    );
    fields = lib.splitString "\t" line;
  in
  builtins.length fields == 31
  &&
    builtins.fromJSON (lib.elemAt fields 30) == {
      schemaVersion = 1;
      requested = [
        "files"
        "shell"
        "terminal"
      ];
      research = false;
      stateQuota = false;
      requireStateQuota = true;
      runtimeImage = {
        kind = "registry";
        reference = definition.image;
      };
    }
  &&
    builtins.fromJSON (lib.elemAt fields 29) == {
      memoryBytes = 512 * 1024 * 1024;
      memorySwapBytes = 512 * 1024 * 1024;
      nanoCpus = 500000000;
      pidsLimit = 64;
      nofile = 128;
      nproc = 64;
    }
  &&
    builtins.fromJSON (lib.elemAt fields 27) == {
      "/run" = 16 * 1024 * 1024;
      "/tmp" = 32 * 1024 * 1024;
      "/var/tmp" = 32 * 1024 * 1024;
    };
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
assert lib.any (lib.hasInfix "workspace quota") (failures genericStarted);
assert lib.any (lib.hasInfix "private state quota") (failures genericStarted);
assert !(lib.any (lib.hasInfix "requires an enabled broker") (failures genericStarted));
assert !(lib.any (lib.hasInfix "requires its isolated Research relay") (failures genericStarted));
assert failures genericOffline == [ ];
assert genericOffline.virtualisation.oci-containers.containers.generic-coding.autoStart;
assert genericOffline.virtualisation.oci-containers.containers.generic-coding.networks == [ ];
assert
  genericOffline.virtualisation.oci-containers.containers.generic-coding.environmentFiles == [ ];
assert
  !(lib.hasInfix "Generic runtime admission refused:" genericOffline.systemd.services.docker-generic-coding.preStart);
assert genericOffline.tentaflake.agentInstances.generic-coding.requireStateQuota;
assert
  generic.tentaflake.agentInstances.generic-coding.requestedCapabilities == [
    "files"
    "shell"
    "terminal"
  ];
assert generic.tentaflake.security.resources.pidsLimit == 512;
assert lib.hasInfix "Generic runtime admission refused: missing workspace-quota, state-quota."
  generic.systemd.services.docker-generic-coding.preStart;
assert failures genericMissing == [ ];
assert lib.hasInfix
  "Generic runtime admission refused: missing model, research, worker, workspace-quota, state-quota."
  genericMissing.systemd.services.docker-generic-coding.preStart;
assert lib.all (fragment: lib.any (lib.hasInfix fragment) (failures genericMissingStarted)) [
  "requires an enabled broker"
  "requires its isolated Research relay"
  "requires an enabled disposable worker"
  "workspace quota"
  "private state quota"
];
assert failures genericWithWorker == [ ];
assert lib.elem
  "/var/lib/tentaflake-worker-generic-coding/results:/run/tentaflake-worker/results:ro"
  genericWithWorker.virtualisation.oci-containers.containers.generic-coding.volumes;
assert lib.any (lib.hasInfix "generic worker declarations must match") (
  failures (genericEval { } genericWorkerPolicy)
);
assert lib.all
  (
    mismatch:
    lib.any (lib.hasInfix "generic worker declarations must match") (
      failures (
        genericEval { capabilities = [ "worker" ]; } (
          lib.recursiveUpdate genericWorkerPolicy {
            tentaflake.worker.agents.generic-coding = mismatch;
          }
        )
      )
    )
  )
  [
    { workspace = "/var/lib/other/workspace"; }
    { containerUid = 10001; }
    { containerGid = 10001; }
  ];
assert lib.all
  (
    mismatch:
    lib.any (lib.hasInfix "quota must match") (
      failures (
        genericEval { } {
          tentaflake.workspaceQuota.agents.generic-coding = genericQuota // mismatch;
        }
      )
    )
  )
  [
    { workspace = "/var/lib/other/workspace"; }
    { ownerUid = 10001; }
    { ownerGid = 10001; }
    {
      state = {
        path = "/var/lib/other/state";
        sizeMiB = 128;
      };
    }
  ];
assert failures genericWithModel == [ ];
assert genericModelContainer.networks == [ "tf-generic-coding" ];
assert !(lib.elem "--network=none" genericModelContainer.extraOptions);
assert
  genericModelContainer.environmentFiles == [ "/run/tentaflake-broker/generic-coding/agent.env" ];
assert lib.any (lib.hasInfix "generic model broker must match") (
  failures (genericEval { } genericModelPolicy)
);
assert
  lib.length (
    lib.filter (lib.hasInfix "host security resource ceilings") (failures excessiveResources)
  ) == builtins.length (builtins.attrNames exceedsHost);
assert lib.any (lib.hasInfix "effective memorySwap") (
  failures (
    genericEval {
      resources.memorySwap = "256m";
    } { }
  )
);
assert lib.all (bad: rejects (runtimeContract (definition // bad))) [
  { schemaVersion = 2; }
  { image = "example.invalid/agent:latest"; }
  {
    image = {
      kind = "local-artifact";
      descriptor = builtins.unsafeDiscardStringContext (toString descriptorLocator);
    };
  }
  {
    image = {
      kind = "local-artifact";
      descriptor = "/tmp/image.json";
    };
  }
  {
    image = {
      kind = "local-artifact";
      descriptor = 1;
    };
  }
  {
    image = localImage // {
      pull = "missing";
    };
  }
  {
    image = localImage // {
      descriptor = "${descriptorLocator}/../other.json";
    };
  }
  {
    image = localImage // {
      kind = "archive";
    };
  }
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
  { capabilities = [ "internet" ]; }
  {
    capabilities = [
      "shell"
      "shell"
    ];
  }
];
assert
  localDefinition.runtimeImage == {
    kind = "local-artifact";
    descriptor = toString descriptorLocator;
  };
assert builtins.hasContext localDefinition.image;
assert localDefinition.image == toString descriptorLocator;
assert failures localGeneric == [ ];
assert localContainer.image == localDefinition.image;
assert localContainer.pull == "never";
assert localContainer.imageFile == null && localContainer.imageStream == null;
assert lib.all (value: value == null) (lib.attrValues localContainer.login);
assert
  localGeneric.tentaflake.agentInstances.generic-coding.runtimeImage == localDefinition.runtimeImage;
assert localManifest.schemaVersion == 3;
assert localManifest.runtimes.generic-coding.image == localDefinition.runtimeImage;
assert localManifest.runtimes.generic-coding.unit == "docker-generic-coding.service";
assert localManifest.runtimes.generic-coding.container.cmd == localContainer.cmd;
assert localManifest.runtimes.generic-coding.container.extraOptions == localContainer.extraOptions;
assert localSupervisor.serviceConfig.Type == "notify";
assert localSupervisor.serviceConfig.Restart == "no";
assert localSupervisor.serviceConfig.NotifyAccess == "main";
assert localSupervisor.serviceConfig.KillMode == "control-group";
assert localSupervisor.serviceConfig.ExecStartPre == [ ];
assert localSupervisor.serviceConfig.ExecStartPost == [ ];
assert localSupervisor.serviceConfig.ExecStopPost == [ ];
assert
  localSupervisor.script == "" && localSupervisor.preStart == "" && localSupervisor.preStop == "";
assert lib.hasInfix "runtime-start" localSupervisor.serviceConfig.ExecStart;
assert lib.hasInfix "runtime-stop" localSupervisor.serviceConfig.ExecStop;
assert !localContainer.autoRemoveOnStop;
assert !localGeneric.virtualisation.docker.daemon.settings.live-restore;
assert !localGeneric.virtualisation.docker.daemon.settings.features.containerd-snapshotter;
assert lib.any (lib.hasInfix "requires the classic Docker image store") (
  failures (
    genericEval { image = localImage; } (
      lib.recursiveUpdate localPolicy {
        virtualisation.docker.daemon.settings.features.containerd-snapshotter = true;
      }
    )
  )
);
assert !localGeneric.systemd.services.docker-prune.enable;
assert lib.any (lib.hasInfix "requires Docker automatic pruning disabled") (
  failures (
    genericEval { image = localImage; } (
      lib.recursiveUpdate localPolicy { virtualisation.docker.autoPrune.enable = true; }
    )
  )
);
assert
  let
    disabledProvenance = genericEval { image = localImage; } (
      lib.recursiveUpdate localPolicy {
        tentaflake.imageProvenance.agents.generic-coding.enable = false;
      }
    );
    disabledPlan = builtins.fromJSON (
      builtins.unsafeDiscardStringContext disabledProvenance.tentaflake.admissionManifest.text
    );
  in
  failures disabledProvenance == [ ] && disabledPlan.runtimes.generic-coding.provenance == null;
assert lib.any (lib.hasInfix "require configured secure Docker admission") (
  failures (genericEval { image = localImage; } { })
);
assert lib.all
  (
    override:
    lib.any (lib.hasInfix "local artifacts forbid registry login or pull fallback") (
      failures (
        genericEval { image = localImage; } (
          lib.recursiveUpdate localPolicy {
            virtualisation.oci-containers.containers.generic-coding = override;
          }
        )
      )
    )
  )
  [
    { pull = lib.mkForce "missing"; }
    { login.registry = "example.invalid"; }
    { imageFile = descriptorLocator; }
  ];
assert lib.any (lib.hasInfix "effective image must match") (
  failures (
    genericEval { image = localImage; } (
      lib.recursiveUpdate localPolicy {
        virtualisation.oci-containers.containers.generic-coding.image = lib.mkForce definition.image;
      }
    )
  )
);
assert lib.all
  (
    provenance:
    lib.any (lib.hasInfix "local image artifacts do not provide registry Cosign provenance") (
      failures (
        genericEval { image = localImage; } (
          lib.recursiveUpdate localPolicy {
            tentaflake.imageProvenance = provenance;
          }
        )
      )
    )
  )
  [
    { requireForSecureAgents = true; }
    {
      agents.generic-coding = {
        mode = "key";
        publicKeyFile = "/etc/fixture.pub";
      };
    }
  ];
assert
  let
    line = lib.findFirst (lib.hasPrefix "agent\tgeneric-coding\t") "" (
      lib.splitString "\n" localGeneric.environment.etc."tentaflake/security.tsv".text
    );
    fields = lib.splitString "\t" line;
    # Inspect values only; generated manifests retain the source dependency.
    declaration = builtins.fromJSON (builtins.unsafeDiscardStringContext (lib.elemAt fields 30));
  in
  lib.elemAt fields 13 == "false"
  && declaration.runtimeImage.kind == "local-artifact"
  && declaration.runtimeImage.descriptor == toString descriptorLocator
  && builtins.hasContext line;
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
assert lib.all
  (
    directory:
    rejects (
      contract "hermes" (
        builders.adapters.hermes
        // {
          layout = builders.adapters.hermes.layout // {
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
(import ./agent-state-quota.nix { inherit pkgs eval builders; })
// (import ./creation-templates.nix { inherit pkgs eval builders; })
// {
  deniedStartScript = genericMissing.systemd.services.docker-generic-coding.preStart;
  admissionPlan = localGeneric.tentaflake.admissionManifest;
}
