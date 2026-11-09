# Pure/module evaluation only: synthetic image proof and unused credential path.
# No provider requests, credential contents, activation or runtime execution.
{ self, pkgs }:
let
  inherit (pkgs) lib;
  validate = import ../lib/systemSchema.nix { inherit lib; };
  derive = import ../lib/piDurableProvisioning.nix { inherit lib; };
  systemFromData = import ../lib/systemFromData.nix { inherit lib pkgs; };
  agentsFromData = import ../lib/agentsFromData.nix { inherit lib pkgs; };
  toolPolicy = import ../lib/piDurableTools.nix { inherit lib; };
  fixture = ./fixtures/pi-durable-provisioning/tentaflake.toml;
  desired = validate (builtins.fromTOML (builtins.readFile fixture));
  package = pkgs.hello.overrideAttrs (old: {
    passthru = (old.passthru or { }) // {
      upstreamRevision = "35dfa757378bd7a8570670e99c2687bdd1039a06";
    };
  });
  image = "tentaflake-pi-durable:${lib.concatStrings (lib.replicate 64 "a")}";
  bundle = pkgs.runCommand "pi-provisioning-synthetic-image-proof" {
    passthru = {
      runtimePackage = package;
      imageVerificationVersion = 1;
      imageSelector = image;
    };
  } "mkdir $out";
  policy = {
    enable = true;
    maxActiveAgents = 3;
    networkOctet = 204;
    allowNonStrictCostEnvelope = true;
    provider = {
      upstreamBaseUrl = "https://127.0.0.1:9443/v1/";
      credentialFile = "/run/pi-provisioning-fixture/unused-provider";
    };
    model = {
      name = "gpt-4.1-mini";
      inputMicrousdPerMillion = 1;
      outputMicrousdPerMillion = 1;
    };
    research = {
      tools = [
        "research_job"
        "research_search"
        "research_fetch"
        "research_browser"
        "research_read"
      ];
      relayUidBase = 1000000;
      relayUidCount = 1000000;
    };
    resources = {
      memoryMiB = 256;
      cpuMillis = 500;
      tmpfsMiB = 16;
      runTmpfsMiB = 16;
      nofile = 1024;
    };
    broker = {
      maxRequestBytes = 65536;
      maxResponseBytes = 1024 * 1024;
      maxConcurrency = 1;
      maxRequestsPerMinute = 4;
      dailyRequestBudget = 10;
      dailyTokenBudget = 10000;
      dailyCostMicrousd = 10000;
      maxCompletionTokens = 1024;
    };
    worker = {
      memoryMiB = 256;
      cpuMillis = 500;
      pidsLimit = 64;
      maxInboxEntries = 8;
      maxInboxMiB = 1;
      maxQueueEntries = 8;
      maxQueueMiB = 1;
      maxRetainedResults = 8;
      maxSnapshotMiB = 8;
      maxSnapshotEntries = 1000;
      maxRetainedResultMiB = 32;
      maxRetainedEntries = 4096;
      maxLogMiB = 1;
      workspaceTmpfsMiB = 16;
      tmpTmpfsMiB = 16;
    };
    quota = {
      workspaceMiB = 128;
      stateMiB = 128;
    };
  };
  hostPolicy = {
    tentaflake = {
      piDurableProvisioning = policy;
      piDurableRuntime = {
        enable = true;
        transport = "runsc-pi-durable-v1";
        inherit package image bundle;
      };
    };
    services.secureResearch = {
      serviceUid = 4201;
      egressUid = 4202;
      vpnInterface = "fixture-vpn";
      resolvers = [ "198.18.0.1" ];
    };
  };
  base = {
    system.stateVersion = "26.05";
    fileSystems."/" = {
      device = "/dev/disk/by-label/nixos";
      fsType = "btrfs";
    };
    boot.loader.grub.devices = [ "nodev" ];
    virtualisation.oci-containers.backend = "docker";
    tentaflake = {
      hostName = "pi-provisioning-fixture";
      adminUser = "operator";
      boot.enable = false;
      locale.enable = false;
      nixSettings.enable = false;
      packages.enable = false;
      shell.enable = false;
      security.profile = "balanced";
      networking.enable = true;
    };
  };
  evaluate =
    source: extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        self.nixosModules.default
        base
        source
      ]
      ++ extra;
    }).config;
  publicSource = systemFromData { file = fixture; };
  fromData = data: { imports = agentsFromData { data = validate data; }; };
  admitted = evaluate publicSource [ hostPolicy ];
  refused = evaluate publicSource [ ];
  changed =
    extra:
    evaluate publicSource [
      hostPolicy
      extra
    ];
  assertionsPass = configuration: lib.all (entry: entry.assertion) configuration.assertions;
  failsClosed =
    configuration:
    let
      attempt = builtins.tryEval (assertionsPass configuration);
    in
    !attempt.success || !attempt.value;
  plan = derive {
    instances = admitted.tentaflake.agentInstances;
    inherit policy;
  };
  first = "agent-research-specialist";
  second = "agent-legal-specialist";
  firstContainer = admitted.virtualisation.oci-containers.containers.${first};
  secondContainer = admitted.virtualisation.oci-containers.containers.${second};
  admittedPolicy = import ../lib/hostApiPolicy.nix {
    config = admitted;
    inherit lib;
  };
  runtimeConfiguration =
    container:
    lib.removeSuffix ":/etc/tentaflake/pi-durable.json:ro" (
      lib.findFirst (lib.hasSuffix ":/etc/tentaflake/pi-durable.json:ro")
        (throw "Pi runtime configuration mount is missing")
        container.volumes
    );
  narrowedPlan = derive {
    instances = lib.mapAttrs (
      _: instance: instance // { tools = [ ]; }
    ) admitted.tentaflake.agentInstances;
    inherit policy;
  };
  largeDesired = validate {
    schemaVersion = 1;
    agents = builtins.genList (i: {
      name = "stopped-${toString i}";
      adapter = "pi-durable";
      autoStart = false;
    }) 300;
  };
  large = evaluate (fromData largeDesired) [ hostPolicy ];
  expandedDesired = desired // {
    # Earlier in both source and lexical order: positional allocation would
    # rekey existing specialists, so all four policy projections must stay put.
    agents = [
      {
        name = "alpha-specialist";
        adapter = "pi-durable";
        autoStart = true;
      }
    ]
    ++ desired.agents;
  };
  expanded = evaluate (fromData expandedDesired) [ hostPolicy ];
  expandedPlan = derive {
    instances = expanded.tentaflake.agentInstances;
    inherit policy;
  };
  manualStopped =
    evaluate
      (fromData (
        desired
        // {
          agents = map (entry: entry // { autoStart = false; }) desired.agents;
        }
      ))
      [
        hostPolicy
        {
          tentaflake = {
            piDurableProvisioning.enable = lib.mkForce false;
            broker.agents = plan.broker;
            research.agents = plan.research;
            worker.agents = plan.worker;
            workspaceQuota.agents = plan.quota;
            security.resources = plan.resources;
          };
        }
      ];
  undeclaredReference = builtins.tryEval (
    builtins.deepSeq (validate {
      schemaVersion = 1;
      inherit (desired) agents;
      research = {
        enable = true;
        agents.agent-unknown.uid = 123456;
      };
    }) true
  );
  crossAgentReference = builtins.tryEval (
    builtins.deepSeq (validate {
      schemaVersion = 1;
      agents = [
        {
          name = "one";
          adapter = "pi-durable";
          autoStart = true;
          policy = second;
        }
      ];
    }) true
  );
  policyMissing = key: changed { tentaflake.piDurableProvisioning.${key} = lib.mkForce null; };
  overCapacity = changed { tentaflake.piDurableProvisioning.maxActiveAgents = lib.mkForce 1; };
  unsupportedTransport = changed {
    tentaflake.piDurableRuntime.transport = lib.mkForce null;
  };
  # Test the pure projection at the public name boundary, not admission: the
  # existing offline-worker transport separately has a shorter runtime limit.
  longName = lib.concatStrings (lib.replicate 57 "x");
  longIdentity = "agent-${longName}";
  longPlan = derive {
    inherit policy;
    instances.${longIdentity} = {
      adapter = "pi-durable";
      autoStart = true;
      workspace = "/var/lib/tentaflake/agents/${longName}/workspace";
      stateStorage = "/var/lib/tentaflake/agents/${longName}/state";
    };
  };
in
# OCI membership must be resolvable before transport admission reads Docker's
# settings/default enablement. Topology is not permission: assertions below
# still reject the same declarations when their transport is not admitted.
assert
  builtins.attrNames admitted.virtualisation.oci-containers.containers == [
    second
    first
  ];
assert
  builtins.attrNames unsupportedTransport.virtualisation.oci-containers.containers == [
    second
    first
  ];
assert !unsupportedTransport.tentaflake.agentInstances.${first}.runnable;
assert !unsupportedTransport.tentaflake.agentInstances.${second}.runnable;
assert assertionsPass admitted;
assert failsClosed refused;
assert plan.count == 2;
assert builtins.stringLength longIdentity == 63;
assert longPlan.broker.${longIdentity}.networkName == longIdentity;
assert builtins.stringLength longPlan.broker.${longIdentity}.networkName <= 63;
assert admitted.tentaflake.agentInstances.${first}.runnable;
assert admitted.tentaflake.agentInstances.${second}.runnable;
assert !admitted.tentaflake.agentInstances.agent-paused-specialist.runnable;
assert admitted.tentaflake.agentInstances.${first}.tools == [ "research_search" ];
assert admitted.tentaflake.agentInstances.${second}.tools == [ ];
assert admitted.tentaflake.agentInstances.agent-paused-specialist.tools == toolPolicy.supported;
assert
  admittedPolicy.${first}.runtime == {
    adapter = "pi-durable";
    tool_selection = true;
    tools = [ "research_search" ];
  };
assert admittedPolicy.${second}.runtime.tools == [ ];
assert admittedPolicy.${first}.research.enabled && admittedPolicy.${second}.worker.enabled;
assert !refused.tentaflake.agentInstances.${first}.runnable;
assert !refused.tentaflake.agentInstances.${second}.runnable;
assert refused.tentaflake.broker.agents == { } && refused.tentaflake.worker.agents == { };
assert narrowedPlan.broker == plan.broker && narrowedPlan.research == plan.research;
assert narrowedPlan.worker == plan.worker && narrowedPlan.quota == plan.quota;
assert firstContainer.autoStart && secondContainer.autoStart;
assert plan.broker.${first}.subnet != plan.broker.${second}.subnet;
assert plan.research.${first}.uid != plan.research.${second}.uid;
assert plan.worker.${first}.workspace != plan.worker.${second}.workspace;
assert plan.quota.${first}.state.path != plan.quota.${second}.state.path;
assert lib.elem "--runtime=runsc-pi-durable" firstContainer.extraOptions;
assert lib.elem "--runtime=runsc-pi-durable" secondContainer.extraOptions;
assert lib.elem
  "/run/tentaflake-pi-durable/${first}/api-token:/run/tentaflake-pi-durable/api-token:ro"
  firstContainer.volumes;
assert
  !(lib.any (
    mount: lib.hasPrefix "/var/lib/tentaflake-worker-${second}/" mount
  ) firstContainer.volumes);
assert
  !(lib.any (
    mount: lib.hasPrefix "/run/tentaflake-research/${second}:" mount
  ) firstContainer.volumes);
assert
  !(lib.any (mount: lib.hasPrefix "${policy.provider.credentialFile}:" mount) firstContainer.volumes);
assert lib.elem "/run/tentaflake-research/${first}:/run/tentaflake-research:ro"
  firstContainer.volumes;
assert
  !(lib.any (
    mount: lib.hasPrefix "/var/lib/tentaflake/agents/legal-specialist/" mount
  ) firstContainer.volumes);
assert
  !(lib.any (
    mount: lib.hasPrefix "/run/tentaflake-pi-durable/${second}/" mount
  ) firstContainer.volumes);
assert admitted.services.secureResearch.enable;
assert admitted.services.secureResearch.containerClients == plan.research;
assert lib.assertMsg (assertionsPass large) (
  "Stopped-fleet assertions failed:\n"
  + lib.concatStringsSep "\n" (
    map (entry: entry.message) (builtins.filter (entry: !entry.assertion) large.assertions)
  )
);
assert builtins.length (builtins.attrNames large.tentaflake.agentInstances) == 300;
assert large.virtualisation.oci-containers.containers == { };
assert large.tentaflake.broker.agents == { };
assert large.tentaflake.worker.agents == { };
assert large.tentaflake.workspaceQuota.agents == { };
assert large.tentaflake.research.agents == { };
assert !large.services.secureResearch.enable;
assert large.services.secureResearch.containerClients == { };
assert lib.all (instance: !instance.runnable) (builtins.attrValues large.tentaflake.agentInstances);
assert assertionsPass expanded;
assert expanded.tentaflake.agentInstances.agent-alpha-specialist.runnable;
assert expanded.tentaflake.agentInstances.agent-alpha-specialist.tools == toolPolicy.supported;
assert manualStopped.tentaflake.agentInstances.${first}.tools == [ "research_search" ];
assert lib.all
  (
    identity:
    expandedPlan.broker.${identity} == plan.broker.${identity}
    && expandedPlan.research.${identity} == plan.research.${identity}
    && expandedPlan.worker.${identity} == plan.worker.${identity}
    && expandedPlan.quota.${identity} == plan.quota.${identity}
  )
  [
    first
    second
  ];
assert assertionsPass manualStopped;
assert manualStopped.tentaflake.agentInstances.${first}.runnable;
assert !manualStopped.virtualisation.oci-containers.containers.${first}.autoStart;
assert !undeclaredReference.success && !crossAgentReference.success;
assert lib.all (key: failsClosed (policyMissing key)) [
  "model"
  "provider"
  "research"
  "resources"
  "broker"
  "worker"
  "quota"
  "maxActiveAgents"
];
assert failsClosed (changed {
  tentaflake.piDurableProvisioning.provider.credentialFile = lib.mkForce "";
});
assert failsClosed unsupportedTransport;
assert failsClosed (changed {
  tentaflake.piDurableRuntime.image = lib.mkForce null;
});
assert failsClosed (changed {
  services.secureResearch.enable = lib.mkForce false;
});
assert failsClosed (changed {
  tentaflake.piDurableRuntime.package = lib.mkForce null;
});
assert failsClosed (changed {
  tentaflake.piDurableRuntime.bundle = lib.mkForce null;
});
assert failsClosed overCapacity;
assert overCapacity.virtualisation.oci-containers.containers == { };
assert failsClosed (changed {
  tentaflake.piDurableProvisioning.allowNonStrictCostEnvelope = lib.mkForce false;
});
assert failsClosed (changed {
  tentaflake.piDurableProvisioning.research.tools = lib.mkForce [ "research_fetch" ];
});
assert failsClosed (changed {
  tentaflake.piDurableProvisioning.research.relayUidCount = lib.mkForce 1;
});
assert failsClosed (changed {
  tentaflake.research.agents.${first}.uid = lib.mkForce plan.research.${second}.uid;
});
assert failsClosed (changed {
  tentaflake.worker.agents.${first}.workspace = lib.mkForce plan.worker.${second}.workspace;
});
assert failsClosed (changed {
  tentaflake.broker.agents.${first}.llm.allowedModels = lib.mkForce [
    (policy.model // { name = "unreviewed-model"; })
  ];
});
assert failsClosed (changed {
  tentaflake.research.agents.agent-unknown.uid = 123456;
});
pkgs.runCommand "tentaflake-pi-durable-provisioning" { } ''
  ${pkgs.python3}/bin/python3 - ${runtimeConfiguration firstContainer} ${runtimeConfiguration secondContainer} ${runtimeConfiguration expanded.virtualisation.oci-containers.containers.agent-alpha-specialist} <<'PY'
  import json
  import sys

  expected = [
      ["research_search"],
      [],
      ["research_job", "research_search", "research_fetch", "research_browser", "research_read", "offline_worker"],
  ]
  for path, tools in zip(sys.argv[1:], expected, strict=True):
      with open(path) as source:
          configuration = json.load(source)
      assert configuration["tools"] == tools
      assert configuration["model"] == "gpt-4.1-mini"
      assert configuration["researchCommand"].endswith("/bin/research-client")
  PY
  touch "$out"
''
