{ nixpkgsPath, researchFlake }:
let
  system = "x86_64-linux";
  pkgs = import nixpkgsPath { inherit system; };
  inherit (pkgs) lib;
  evalConfig = import (nixpkgsPath + "/nixos/lib/eval-config.nix");
  eval =
    modules:
    evalConfig {
      inherit system modules;
      specialArgs = { inherit researchFlake; };
    };
  builders = import ../lib { inherit pkgs lib; };
  containerSecurity = import ../lib/containerSecurity.nix { inherit lib; };
  hostModule = profile: {
    system.stateVersion = "26.05";
    fileSystems."/" = {
      device = "/dev/disk/by-label/nixos";
      fsType = "btrfs";
    };
    boot.loader.grub.devices = [ "nodev" ];
    virtualisation.oci-containers.backend = "docker";
    tentaflake = {
      hostName = "eval-host";
      adminUser = "operator";
      security.profile = profile;
      boot.enable = false;
      hardening.enable = profile != "dev";
      locale.enable = false;
      networking.enable = false;
      nixSettings.enable = false;
      packages.enable = false;
      tailscale.enable = profile != "dev";
      shell.enable = false;
    };
  };

  core = eval [
    ../modules/default.nix
    (hostModule "dev")
    { tentaflake.shell.enable = lib.mkForce true; }
  ];

  customWorkerImage = eval [
    ../modules/default.nix
    (hostModule "dev")
    {
      tentaflake.worker.image = pkgs.dockerTools.buildLayeredImage {
        name = "fixture-worker";
        tag = "fixture-version";
      };
    }
  ];

  watchdog = eval [
    ../modules/default.nix
    (hostModule "dev")
    {
      tentaflake.hardening.watchdog = {
        enable = true;
        device = "/dev/watchdog0";
        runtimeSec = "45s";
        rebootSec = "5min";
      };
      tentaflake.hardening.enable = lib.mkForce true;
    }
  ];

  capsule = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "hermes-fixture";
      autoStart = false;
    })
    (builders.mkZeroClawAgent {
      name = "zeroclaw-fixture";
      autoStart = false;
    })
  ];
  hermesCapsule = capsule.config.virtualisation.oci-containers.containers."hermes-hermes-fixture";
  zeroclawCapsule =
    capsule.config.virtualisation.oci-containers.containers."zeroclaw-zeroclaw-fixture";
  capsuleFlags = container: container.extraOptions;
  capsuleManifest = capsule.config.environment.etc."tentaflake/security.tsv".text;
  capsuleAttempt = builtins.tryEval capsule.config.system.build.toplevel.drvPath;

  authKeyHost = eval [
    ../modules/default.nix
    (hostModule "balanced")
    { services.tailscale.authKeyFile = "/run/agenix/tailscale-auth"; }
  ];

  provenanceCapsule = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "signed";
      autoStart = false;
    })
    {
      tentaflake.imageProvenance = {
        requireForSecureAgents = true;
        agents.hermes-signed = {
          mode = "keyless";
          certificateIdentity = "https://github.com/example/project/.github/workflows/release.yml@refs/tags/v1.2.3";
          certificateOidcIssuer = "https://token.actions.githubusercontent.com";
        };
      };
    }
  ];
  provenanceAttempt = builtins.tryEval provenanceCapsule.config.system.build.toplevel.drvPath;
  provenanceService =
    provenanceCapsule.config.systemd.services."tentaflake-image-verify-hermes-signed";

  missingProvenanceCapsule = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "unsigned";
      autoStart = false;
    })
    { tentaflake.imageProvenance.requireForSecureAgents = true; }
  ];
  missingProvenanceAttempt = builtins.tryEval missingProvenanceCapsule.config.system.build.toplevel.drvPath;

  podmanCapsule = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      virtualisation.oci-containers.backend = lib.mkForce "podman";
      tentaflake = {
        worker.agents.hermes-podman-fixture = {
          enable = true;
          workspace = "/var/lib/hermes-podman-fixture/workspace";
        };
        networking.enable = lib.mkForce true;
        broker.agents.hermes-podman-fixture = {
          enable = true;
          networkName = "tf-hermes-podman";
          subnet = "10.203.40.0/30";
          gateway = "10.203.40.1";
          fetch = {
            enable = true;
            allowedHosts = [ "docs.example.com" ];
          };
        };
      };
    }
    (builders.mkHermesAgent {
      name = "podman-fixture";
      autoStart = false;
    })
  ];
  podmanContainer =
    podmanCapsule.config.virtualisation.oci-containers.containers."hermes-podman-fixture";
  podmanAttempt = builtins.tryEval podmanCapsule.config.system.build.toplevel.drvPath;

  strict = eval [
    ../modules/default.nix
    (hostModule "strict")
  ];
  strictAttempt = builtins.tryEval strict.config.system.build.toplevel.drvPath;

  unlimitedResourceFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.security.resources.memory = "0";
    }
  ];
  unlimitedResourceAttempt = builtins.tryEval unlimitedResourceFixture.config.system.build.toplevel.drvPath;

  oversizedControllerMemoryFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.security.resources = {
        memory = "1025g";
        memorySwap = "1025g";
      };
    }
  ];
  oversizedControllerMemoryAttempt = builtins.tryEval oversizedControllerMemoryFixture.config.system.build.toplevel.drvPath;

  oversizedControllerTmpfsFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.security.resources.tmpfsSize = "65g";
    }
  ];
  oversizedControllerTmpfsAttempt = builtins.tryEval oversizedControllerTmpfsFixture.config.system.build.toplevel.drvPath;

  oversizedControllerCpuFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.security.resources.cpus = "1025";
    }
  ];
  oversizedControllerCpuAttempt = builtins.tryEval oversizedControllerCpuFixture.config.system.build.toplevel.drvPath;

  overpreciseControllerCpuFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.security.resources.cpus = "0.000001";
    }
  ];
  overpreciseControllerCpuAttempt = builtins.tryEval overpreciseControllerCpuFixture.config.system.build.toplevel.drvPath;

  missingManagementFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.tailscale.enable = lib.mkForce false;
    }
  ];
  missingManagementAttempt = builtins.tryEval missingManagementFixture.config.system.build.toplevel.drvPath;

  publicSshFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.ssh.enable = true;
    }
  ];
  publicSshAttempt = builtins.tryEval publicSshFixture.config.system.build.toplevel.drvPath;

  gitFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "git";
      autoStart = false;
      gitAutoPush = {
        tokenEnvFile = "/run/agenix/github-agent";
        allowedRemotes = [ "https://github.com/example/agent-repo.git" ];
        allowedBranches = [ "main" ];
      };
    })
  ];
  gitScript = gitFixture.config.systemd.services."hermes-git-autopush".script;
  gitAttempt = builtins.tryEval gitFixture.config.system.build.toplevel.drvPath;

  unsafeGitFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "unsafe-git";
      autoStart = false;
      gitAutoPush.tokenEnvFile = "/run/agenix/github-agent";
    })
  ];
  unsafeGitAttempt = builtins.tryEval unsafeGitFixture.config.system.build.toplevel.drvPath;

  unsafeGitRootFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "unsafe-git-root";
      autoStart = false;
      gitAutoPush = {
        tokenEnvFile = "/run/agenix/github-agent";
        allowedRemotes = [ "https://github.com/example/agent-repo.git" ];
        reposRoot = "/home";
      };
    })
  ];
  unsafeGitRootAttempt = builtins.tryEval unsafeGitRootFixture.config.system.build.toplevel.drvPath;

  unsafePolicy = containerSecurity.apply {
    profile = "balanced";
    backend = "docker";
    name = "unsafe-fixture";
    owner = "10000:10000";
    baseConfig = {
      image = "example.invalid/image@sha256:fixture";
      volumes = [ "/var/lib/fixture:/state:rw" ];
      privileged = true;
    };
    allowedWritableSources = [ "/var/lib/fixture" ];
    allowedWritableDestinations = [ "/state" ];
    pidsLimit = 32;
    resources = {
      memory = "128m";
      memorySwap = "128m";
      cpus = "0.5";
      nofile = 128;
      tmpfsSize = "16m";
      runTmpfsSize = "8m";
    };
  };
  policyFixture =
    overrides:
    containerSecurity.apply {
      profile = "balanced";
      backend = "docker";
      name = "policy-fixture";
      owner = "10000:10000";
      baseConfig = {
        image = "example.invalid/image@sha256:fixture";
        volumes = [ "/var/lib/fixture:/state:rw" ];
      };
      inherit overrides;
      allowedWritableSources = [ "/var/lib/fixture" ];
      allowedWritableDestinations = [ "/state" ];
      pidsLimit = 32;
      resources = {
        memory = "128m";
        memorySwap = "128m";
        cpus = "0.5";
        nofile = 128;
        tmpfsSize = "16m";
        runTmpfsSize = "8m";
      };
    };
  unsafeEnvironmentPolicy = policyFixture {
    environmentFiles = [ "/run/secrets/provider.env" ];
  };
  unsafeOptionPolicy = policyFixture {
    extraOptions = [ "--userns=host" ];
  };
  unsafeGpuPolicy = policyFixture {
    extraOptions = [ "--gpus=all" ];
  };
  unsafeMountPolicy = policyFixture {
    volumes = [ "/etc:/host-etc:ro" ];
  };
  unsafeSharedMountPolicy = policyFixture {
    volumes = [ "/srv/shared:/shared:ro" ];
  };
  unsafeImageArchivePolicy = policyFixture {
    imageFile = pkgs.writeText "unreviewed-image" "fixture";
  };
  unsafeDevNamePolicy = containerSecurity.apply {
    profile = "dev";
    backend = "docker";
    name = "bad;name";
    owner = "10000:10000";
    baseConfig = { };
    allowedWritableSources = [ ];
    allowedWritableDestinations = [ ];
    pidsLimit = null;
    resources = {
      memory = "128m";
      memorySwap = "128m";
      cpus = "0.5";
      nofile = 128;
      tmpfsSize = "16m";
      runTmpfsSize = "8m";
    };
  };

  mutableImageFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "mutable";
      autoStart = false;
      image = "example.invalid/hermes:latest";
      allowMutableImage = true;
    })
  ];
  mutableImageAttempt = builtins.tryEval mutableImageFixture.config.system.build.toplevel.drvPath;

  publishedPortFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkZeroClawAgent {
      name = "published";
      autoStart = false;
      hostPort = 4096;
    })
  ];
  publishedPortAttempt = builtins.tryEval publishedPortFixture.config.system.build.toplevel.drvPath;

  rootHermesFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "root-hermes";
      autoStart = false;
      containerUid = 0;
      containerGid = 0;
    })
  ];
  rootHermesAttempt = builtins.tryEval rootHermesFixture.config.system.build.toplevel.drvPath;

  unsafeHealFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "unsafe-heal";
      autoStart = false;
      healDataDirs = [ "/etc" ];
    })
  ];
  unsafeHealAttempt = builtins.tryEval unsafeHealFixture.config.system.build.toplevel.drvPath;

  seedFixture = pkgs.writeTextDir "SOUL.md" "fixture";
  seededCapsule = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "seeded-hermes";
      autoStart = false;
      seedDir = seedFixture;
    })
    (builders.mkZeroClawAgent {
      name = "seeded-zero";
      autoStart = false;
      seedDir = seedFixture;
    })
  ];
  seededAttempt = builtins.tryEval seededCapsule.config.system.build.toplevel.drvPath;

  devFixture = eval [
    ../modules/default.nix
    (hostModule "dev")
    (builders.mkHermesAgent {
      name = "dev";
      autoStart = false;
      envFile = "/run/tentaflake/dev.env";
      networkMode = "host";
    })
  ];
  devContainer = devFixture.config.virtualisation.oci-containers.containers."hermes-dev";

  brokerCapsule = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "brokered";
      autoStart = false;
    })
    {
      tentaflake.networking.enable = lib.mkForce true;
      tentaflake.broker.agents.hermes-brokered = {
        enable = true;
        networkName = "tf-hermes-brokered";
        subnet = "10.203.20.0/30";
        gateway = "10.203.20.1";
        llm = {
          enable = true;
          upstreamBaseUrl = "https://api.example.com/v1/";
          providerCredentialFile = "/run/agenix/example-provider";
          allowedModels = [
            {
              name = "example/model";
              inputMicrousdPerMillion = 1000000;
              outputMicrousdPerMillion = 2000000;
            }
          ];
        };
        fetch = {
          enable = true;
          allowedHosts = [ "docs.example.com" ];
        };
      };
    }
  ];
  brokerContainer = brokerCapsule.config.virtualisation.oci-containers.containers."hermes-brokered";
  brokerBridge = "tfb-${builtins.substring 0 8 (builtins.hashString "sha256" "hermes-brokered")}";
  brokerManifest = brokerCapsule.config.environment.etc."tentaflake/security.tsv".text;
  brokerAttempt = builtins.tryEval brokerCapsule.config.system.build.toplevel.drvPath;

  brokerProjection =
    (brokerCapsule.extendModules {
      modules = [
        {
          networking.firewall.allowedTCPPorts = [ 2222 ];
          tentaflake.broker.agents = {
            hermes-brokered = {
              llm.port = 7910;
              fetch.port = 7810;
            };
            disabled = {
              subnet = "10.203.72.0/30";
              gateway = "10.203.72.1";
              llm.enable = true;
            };
          };
        }
      ];
    }).config;
  brokerModel = brokerProjection.tentaflake.broker.admissionModels.hermes-brokered;
  brokerBoundary = brokerProjection.networking.nftables.tables.${brokerModel.firewallTable};

  streamingCapsule = brokerCapsule.extendModules {
    modules = [
      {
        tentaflake.broker.agents.hermes-brokered.llm.streaming = {
          enable = true;
          maxEventBytes = 8192;
          firstEventTimeoutSeconds = 5;
          idleTimeoutSeconds = 10;
          totalTimeoutSeconds = 30;
        };
      }
    ];
  };
  streamingAssertions =
    limits:
    let
      fixture = streamingCapsule.extendModules {
        modules = [ { tentaflake.broker.agents.hermes-brokered.llm.streaming = lib.mkForce limits; } ];
      };
    in
    lib.all (item: item.assertion) fixture.config.assertions;

  # Exercise the generated services' budgets, including disabled agents.
  brokerHostBudgetAccepted =
    option: value:
    let
      fixture = brokerCapsule.extendModules {
        modules = [ { tentaflake.broker.${option} = value; } ];
      };
    in
    lib.all (item: item.assertion) fixture.config.assertions;
  controllerResourcesAccepted =
    resources:
    let
      fixture = capsule.extendModules {
        modules = [ { tentaflake.security.resources = resources; } ];
      };
    in
    lib.all (item: item.assertion) fixture.config.assertions;
  brokerModeBudgetsAccepted =
    modes: concurrency: rate:
    let
      fixture = brokerCapsule.extendModules {
        modules = [
          {
            tentaflake.broker = {
              maxTotalConcurrency = concurrency;
              maxTotalRequestsPerMinute = rate;
              agents = lib.mkForce (
                builtins.listToAttrs (
                  lib.imap0 (index: mode: {
                    name = "mode-${toString index}";
                    value = brokerCapsule.config.tentaflake.broker.agents.hermes-brokered // {
                      inherit (mode) enable;
                      maxConcurrency = 4;
                      maxRequestsPerMinute = 30;
                      llm = brokerCapsule.config.tentaflake.broker.agents.hermes-brokered.llm // {
                        enable = mode.llm;
                      };
                      fetch = brokerCapsule.config.tentaflake.broker.agents.hermes-brokered.fetch // {
                        enable = mode.fetch;
                      };
                    };
                  }) modes
                )
              );
            };
          }
        ];
      };
      # Successful NixOS assertions may carry lazy messages that are only
      # valid on failure (for example Docker containers' unused Podman user).
      failures = lib.filter (item: !item.assertion) fixture.config.assertions;
    in
    lib.all (
      item:
      !(lib.hasInfix "maxTotalConcurrency" item.message)
      && !(lib.hasInfix "maxTotalRequestsPerMinute" item.message)
    ) failures;
  llmMode = {
    enable = true;
    llm = true;
    fetch = false;
  };
  fetchMode = {
    enable = true;
    llm = false;
    fetch = true;
  };
  dualMode = {
    enable = true;
    llm = true;
    fetch = true;
  };

  brokerAggregateBudgetFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "broker-overbudget";
      autoStart = false;
    })
    {
      tentaflake = {
        networking.enable = lib.mkForce true;
        broker = {
          maxTotalConcurrency = 3;
          agents.hermes-broker-overbudget = {
            enable = true;
            subnet = "10.203.24.0/30";
            gateway = "10.203.24.1";
            fetch = {
              enable = true;
              allowedHosts = [ "docs.example.com" ];
            };
          };
        };
      };
    }
  ];
  brokerAggregateBudgetAttempt = builtins.tryEval brokerAggregateBudgetFixture.config.system.build.toplevel.drvPath;

  workerCapsule = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "worker";
      autoStart = true;
    })
    (builders.mkZeroClawAgent {
      name = "worker";
      autoStart = false;
    })
    {
      services.secureResearch = {
        serviceUid = 4201;
        egressUid = 4202;
        vpnInterface = "fixture-vpn";
        resolvers = [ "9.9.9.9" ];
      };
      tentaflake = {
        research.agents.hermes-worker.uid = 62101;
        networking.enable = lib.mkForce true;
        broker.agents.hermes-worker = {
          enable = true;
          networkName = "tf-hermes-worker";
          subnet = "10.203.22.0/30";
          gateway = "10.203.22.1";
          llm = {
            enable = true;
            upstreamBaseUrl = "https://api.example.org/v1/";
            providerCredentialFile = "/run/fixture-model-key";
            allowedModels = [
              {
                name = "fixture-model";
                inputMicrousdPerMillion = 1;
                outputMicrousdPerMillion = 1;
              }
            ];
          };
        };
        worker.agents = {
          hermes-worker = {
            enable = true;
            workspace = "/var/lib/hermes-worker/workspace";
          };
          zeroclaw-worker = {
            enable = true;
            workspace = "/var/lib/zeroclaw-worker/data";
            containerUid = 65534;
            containerGid = 65534;
          };
        };
        workspaceQuota.agents.hermes-worker = {
          enable = true;
          workspace = "/var/lib/hermes-worker/workspace";
          sizeMiB = 128;
        };
      };
    }
  ];
  workerContainer = workerCapsule.config.virtualisation.oci-containers.containers.hermes-worker;
  workerService = workerCapsule.config.systemd.services.tentaflake-worker-hermes-worker;
  numericWorkerService = workerCapsule.config.systemd.services.tentaflake-worker-zeroclaw-worker;
  workerPath = workerCapsule.config.systemd.paths.tentaflake-worker-hermes-worker;
  workerOwnerService = workerCapsule.config.systemd.services.tentaflake-workspace-quota-hermes-worker;
  workerQuotaHelpers =
    workerCapsule.config.tentaflake.admissionVolumes."workspace:hermes-worker".helpers;
  workerPrepareLines = lib.splitString "\n" workerCapsule.config.systemd.services.tentaflake-workspace-quota-prepare-hermes-worker.script;
  workerAttempt = builtins.tryEval workerCapsule.config.system.build.toplevel.drvPath;
  guardedWorker = workerCapsule.extendModules {
    modules = [
      {
        tentaflake.security.admission = {
          authorityEpoch = "worker-module-fixture";
          capacity = {
            memoryBytes = 4294967296;
            memorySwapBytes = 4294967296;
            nanoCpus = 4000000000;
            pids = 1024;
            workspaceBytes = 1073741824;
            stateBytes = 1073741824;
          };
          hostReserve = lib.genAttrs [
            "memoryBytes"
            "memorySwapBytes"
            "nanoCpus"
            "pids"
            "workspaceBytes"
            "stateBytes"
          ] (_: 0);
          researchReserve = lib.genAttrs [
            "memoryBytes"
            "memorySwapBytes"
            "nanoCpus"
            "pids"
            "workspaceBytes"
            "stateBytes"
          ] (_: 0);
          maxActive = 2;
        };
      }
    ];
  };
  guardedWorkerPlan = builtins.fromJSON (
    builtins.unsafeDiscardStringContext guardedWorker.config.tentaflake.admissionManifest.text
  );
  guardedWorkerSpec = guardedWorkerPlan.workers.hermes-worker;
  guardedWorkerService = guardedWorker.config.systemd.services.tentaflake-worker-hermes-worker;
  legacyWorkerInputs = workerCapsule.config.tentaflake.worker.maintenance;
  podmanWorkerInputs = podmanCapsule.config.tentaflake.worker.maintenance;
  reservedWorkerNames =
    map
      (
        name:
        workerCapsule.extendModules {
          modules = [
            {
              virtualisation.oci-containers.containers.${name} = {
                image = builders.constants.hermesImage;
                autoStart = false;
                volumes = [ "/var/lib/${name}/workspace:/workspace" ];
              };
              tentaflake.worker.agents.${name} = {
                enable = true;
                workspace = "/var/lib/${name}/workspace";
              };
            }
          ];
        }
      )
      [
        "image"
        "state-volumes"
      ];
  tooSmallQuota = workerCapsule.extendModules {
    modules = [ { tentaflake.workspaceQuota.agents.hermes-worker.sizeMiB = lib.mkForce 32; } ];
  };
  workerMount = lib.findFirst (
    mount: mount.where == "/var/lib/hermes-worker/workspace"
  ) null workerCapsule.config.systemd.mounts;

  unsafeWorkerFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "unsafe-worker";
      autoStart = false;
    })
    {
      tentaflake.worker.agents.hermes-unsafe-worker = {
        enable = true;
        workspace = "/var/lib/a-different-workspace";
      };
    }
  ];
  unsafeWorkerAttempt = builtins.tryEval unsafeWorkerFixture.config.system.build.toplevel.drvPath;

  incompleteAutostartFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "incomplete-autostart";
      autoStart = true;
    })
  ];
  incompleteAutostartAttempt = builtins.tryEval incompleteAutostartFixture.config.system.build.toplevel.drvPath;

  unsafeQuotaFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkZeroClawAgent {
      name = "unsafe-quota";
      autoStart = false;
    })
    {
      tentaflake.workspaceQuota.agents.zeroclaw-unsafe-quota = {
        enable = true;
        workspace = "/var/lib/not-zeroclaw-data";
        ownerUid = 65534;
        ownerGid = 65534;
      };
    }
  ];
  unsafeQuotaAttempt = builtins.tryEval unsafeQuotaFixture.config.system.build.toplevel.drvPath;

  unsafeBrokerFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "unsafe-broker";
      autoStart = false;
    })
    {
      tentaflake.networking.enable = lib.mkForce true;
      tentaflake.broker.agents.hermes-unsafe-broker = {
        enable = true;
        subnet = "10.203.21.0/30";
        gateway = "10.203.99.1";
        fetch.enable = true;
        fetch.allowedHosts = [ "*.example.com" ];
      };
    }
  ];
  unsafeBrokerAttempt = builtins.tryEval unsafeBrokerFixture.config.system.build.toplevel.drvPath;

  backupWithPaths =
    paths:
    eval [
      ../modules/default.nix
      (hostModule "balanced")
      (builders.mkHermesAgent {
        name = "fixture";
        autoStart = false;
      })
      (builders.mkHermesAgent {
        name = "fixture-other";
        autoStart = false;
      })
      {
        tentaflake.workspaceQuota.agents = {
          hermes-fixture = {
            enable = true;
            workspace = "/var/lib/hermes-fixture/workspace";
          };
          hermes-fixture-other = {
            enable = true;
            workspace = "/var/lib/hermes-fixture-other/workspace";
          };
          disabled.workspace = "/var/lib/hermes-fixture/disabled";
        };
        tentaflake.backup = {
          inherit paths;
          enable = true;
          repositoryFile = "/run/credentials/restic-repository";
          passwordFile = "/run/credentials/restic-password";
        };
      }
    ];
  backup = backupWithPaths [ "/var/lib/hermes-fixture/" ];
  explicitWorkspaceBackup = backupWithPaths [ "/var/lib/hermes-fixture/workspace" ];
  unrelatedBackup = backupWithPaths [ "/var/lib/unrelated" ];
  backupAttempt = builtins.tryEval backup.config.system.build.toplevel.drvPath;

  unsafeBackup = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.backup = {
        enable = true;
        paths = [ "/" ];
        repositoryFile = "/etc/restic-repository";
        passwordFile = "/tmp/restic-password";
      };
    }
  ];
  unsafeBackupAttempt = builtins.tryEval unsafeBackup.config.system.build.toplevel.drvPath;

  observability = eval [
    ../modules/default.nix
    ../modules/profiles/observability.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "observed";
      autoStart = false;
    })
    {
      tentaflake = {
        profiles.observability = {
          enable = true;
          grafanaSecretKeyFile = "/run/credentials/grafana-secret-key";
          grafanaAdminPasswordFile = "/run/credentials/grafana-admin-password";
        };
        networking.enable = lib.mkForce true;
        broker.agents.hermes-observed = {
          enable = true;
          networkName = "tf-hermes-observed";
          subnet = "10.203.30.0/30";
          gateway = "10.203.30.1";
          llm = {
            enable = true;
            upstreamBaseUrl = "https://api.example.com/v1/";
            providerCredentialFile = "/run/agenix/example-provider";
            allowedModels = [
              {
                name = "example/model";
                inputMicrousdPerMillion = 1;
                outputMicrousdPerMillion = 1;
              }
            ];
          };
        };
      };
    }
  ];

  falcoPackage = pkgs.runCommand "falco-eval-fixture" { } ''
    mkdir -p $out/bin $out/etc/falco
    touch $out/bin/falco $out/etc/falco/falco.yaml
  '';
  falco = eval [
    ../modules/profiles/falco.nix
    {
      system.stateVersion = "26.05";
      tentaflake.profiles.falco = {
        enable = true;
        package = falcoPackage;
      };
    }
  ];
  tailscalePolicy = builtins.fromJSON (builtins.readFile ../docs/tailscale-policy.example.json);
  escapedReference =
    volume:
    eval [
      ../modules/default.nix
      (hostModule "balanced")
      (builders.mkHermesAgent {
        name = "escaped-reference";
        autoStart = false;
        extraVolumes = [ volume ];
      })
    ];
  escapedStoreSource = builtins.tryEval (escapedReference "/nix/store/../../run/fixture-secret:/reference:ro")
  .config.system.build.toplevel.drvPath;
  escapedMountDestination = builtins.tryEval (escapedReference "${pkgs.hello}:/reference/../../etc:ro")
  .config.system.build.toplevel.drvPath;

in
assert
  customWorkerImage.config.tentaflake.worker.imageReference == "fixture-worker:fixture-version";
assert !escapedStoreSource.success;
assert !escapedMountDestination.success;
assert core.config.environment.etc."tentaflake/cli.conf".text != "";
assert core.config.environment.etc."tentaflake/agents.tsv".text == "";
assert watchdog.config.systemd.settings.Manager.WatchdogDevice == "/dev/watchdog0";
assert watchdog.config.systemd.settings.Manager.RuntimeWatchdogSec == "45s";
assert watchdog.config.systemd.settings.Manager.RebootWatchdogSec == "5min";
assert !(core.options.tentaflake ? profiles);
assert !(core.options.tentaflake ? editor);
assert !(builtins.hasAttr "hive-research" core.options.services);
assert !(builtins.hasAttr "piper-tts-server" core.options.services);
assert capsuleAttempt.success;
assert lib.all
  (
    unit:
    unit.startLimitIntervalSec == 0
    && unit.serviceConfig.Restart == "on-failure"
    && unit.serviceConfig.RestartSec == "10s"
    && unit.serviceConfig.RestartSteps == 5
    && unit.serviceConfig.RestartMaxDelaySec == "1min"
  )
  [
    capsule.config.systemd.services.docker-hermes-hermes-fixture
    capsule.config.systemd.services.docker-zeroclaw-zeroclaw-fixture
    brokerCapsule.config.systemd.services.tentaflake-broker-llm-hermes-brokered
    brokerCapsule.config.systemd.services.tentaflake-broker-fetch-hermes-brokered
  ];

assert lib.all
  (
    unit:
    unit.serviceConfig.MemoryMax == 128 * 1024 * 1024
    && unit.serviceConfig.TasksMax == 64
    && unit.serviceConfig.LimitNOFILE == 4096
  )
  [
    brokerCapsule.config.systemd.services.tentaflake-broker-llm-hermes-brokered
    brokerCapsule.config.systemd.services.tentaflake-broker-fetch-hermes-brokered
  ];
assert
  let
    credentials =
      brokerCapsule.config.systemd.services.tentaflake-broker-credentials-hermes-brokered.serviceConfig;
  in
  credentials.RuntimeDirectory == "tentaflake-broker/hermes-brokered"
  && credentials.RuntimeDirectoryMode == "0700"
  && credentials.RuntimeDirectoryPreserve == "yes"
  && credentials.ProtectSystem == "strict"
  && credentials.CapabilityBoundingSet == ""
  && credentials.ReadOnlyPaths == [ "/run/tentaflake-broker" ]
  && credentials.ReadWritePaths == [ "/run/tentaflake-broker/hermes-brokered" ];
assert
  let
    agentLines = lib.filter (lib.hasPrefix "agent\t") (lib.splitString "\n" capsuleManifest);
    fields = lib.splitString "\t" (builtins.unsafeDiscardStringContext (lib.head agentLines));
    resources = builtins.fromJSON (lib.elemAt fields 29);
  in
  lib.length fields == 31
  &&
    builtins.fromJSON (lib.elemAt fields 30) == {
      schemaVersion = 1;
      requested = [
        "model"
        "research"
        "worker"
      ];
      research = false;
      stateQuota = false;
      requireStateQuota = false;
    }
  && builtins.fromJSON (lib.elemAt fields 26) == hermesCapsule.volumes
  &&
    builtins.fromJSON (lib.elemAt fields 27) == {
      "/run" = 64 * 1024 * 1024;
      "/tmp" = 256 * 1024 * 1024;
      "/var/tmp" = 256 * 1024 * 1024;
    }
  && lib.elemAt fields 28 == "true"
  && resources.memoryBytes == 2 * 1024 * 1024 * 1024
  && resources.memorySwapBytes == resources.memoryBytes
  && resources.nanoCpus == 2000000000
  && resources.pidsLimit == 512
  && resources.nproc == resources.pidsLimit
  && resources.nofile == 4096;
assert provenanceAttempt.success;
assert !missingProvenanceAttempt.success;
assert lib.hasInfix "cosign verify --certificate-identity"
  provenanceService.serviceConfig.ExecStart;
assert lib.elem "tentaflake-image-verify-hermes-signed.service"
  provenanceCapsule.config.systemd.services.docker-hermes-signed.requires;
assert podmanAttempt.success;
assert !strictAttempt.success;
assert !unlimitedResourceAttempt.success;
assert !oversizedControllerMemoryAttempt.success;
assert !oversizedControllerTmpfsAttempt.success;
assert !oversizedControllerCpuAttempt.success;
assert !overpreciseControllerCpuAttempt.success;
assert !missingManagementAttempt.success;
assert !publicSshAttempt.success;
assert gitAttempt.success;
assert !unsafeGitAttempt.success;
assert !unsafeGitRootAttempt.success;
assert lib.hasInfix "remote-check" gitScript;
assert lib.hasInfix "safe.directory=\"$repo\"" gitScript;
assert !(lib.hasInfix "safe.directory='*'" gitScript);
assert !(lib.hasInfix "case \"$url\" in *github.com*" gitScript);
assert capsule.config.tentaflake.security.profile == "balanced";
assert import ./management-policy.nix { inherit pkgs; };
assert lib.hasPrefix "manifest\t3\n" capsuleManifest;
assert lib.hasInfix "host\tbalanced\tfalse\tfalse\ttrue\ttrue\tfalse\t36\n" capsuleManifest;
assert lib.hasInfix "management\ttailscale\ttrue\ttrue\ttailnet-policy" capsuleManifest;
assert lib.hasInfix "agent\thermes-hermes-fixture\tbalanced" capsuleManifest;
assert
  lib.getExe' pkgs.gvisor "runsc"
  == capsule.config.virtualisation.docker.daemon.settings.runtimes.runsc.path;
assert
  podmanContainer.preRunExtraOptions == [
    "--runtime"
    "runsc"
  ];
assert lib.elem pkgs.gvisor podmanCapsule.config.virtualisation.podman.extraRuntimes;
assert
  podmanCapsule.config.systemd.services.tentaflake-worker-image.serviceConfig.ExecStart
  == "${guardedWorkerPlan.tools.cli} admission worker-load ${podmanWorkerInputs.image.config}";
assert podmanContainer.networks == [ "tf-hermes-podman" ];
assert lib.hasInfix "--interface-name \"$bridge\""
  podmanCapsule.config.systemd.services.tentaflake-broker-network-hermes-podman-fixture.script;
assert !(lib.all (item: item.assertion) unsafePolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeEnvironmentPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeOptionPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeGpuPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeMountPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeSharedMountPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeImageArchivePolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeDevNamePolicy.assertions);
assert !mutableImageAttempt.success;
assert !publishedPortAttempt.success;
assert !rootHermesAttempt.success;
assert !unsafeHealAttempt.success;
assert seededAttempt.success;
assert
  seededCapsule.config.systemd.services."seed-hermes-seeded-hermes".serviceConfig.User == "10000";
assert
  seededCapsule.config.systemd.services."seed-zeroclaw-seeded-zero".serviceConfig.User == "65534";
assert containerSecurity.containsSensitiveValue { OPENAI_API_KEY = "fixture"; };
assert !(containerSecurity.containsSensitiveValue { MONKEY = "fixture"; });
assert lib.elem "--network=host" devContainer.extraOptions;
assert lib.elem "--env-file=/run/tentaflake/dev.env" devContainer.extraOptions;
assert !(lib.elem "--runtime=runsc" devContainer.extraOptions);
assert brokerAttempt.success;
assert !brokerCapsule.config.tentaflake.broker.agents.hermes-brokered.llm.streaming.enable;
assert lib.all (item: item.assertion) streamingCapsule.config.assertions;
assert streamingAssertions {
  enable = true;
  maxEventBytes = 8192;
  firstEventTimeoutSeconds = 5;
  idleTimeoutSeconds = 10;
  totalTimeoutSeconds = 30;
};
assert
  !streamingAssertions {
    enable = true;
    maxEventBytes = 9 * 1024 * 1024;
    firstEventTimeoutSeconds = 5;
    idleTimeoutSeconds = 10;
    totalTimeoutSeconds = 30;
  };
assert
  !streamingAssertions {
    enable = true;
    maxEventBytes = 8192;
    firstEventTimeoutSeconds = 31;
    idleTimeoutSeconds = 10;
    totalTimeoutSeconds = 30;
  };
assert brokerHostBudgetAccepted "maxEnabledAgents" 1;
assert brokerHostBudgetAccepted "maxTotalDailyTokenBudget" 1000000;
assert !(brokerHostBudgetAccepted "maxTotalDailyTokenBudget" 999999);
assert brokerHostBudgetAccepted "maxTotalDailyCostMicrousd" 10000000;
assert !(brokerHostBudgetAccepted "maxTotalDailyCostMicrousd" 9999999);
assert controllerResourcesAccepted {
  memory = "1t";
  memorySwap = "1t";
  tmpfsSize = "64g";
  runTmpfsSize = "64g";
  cpus = "1024";
};
assert controllerResourcesAccepted { cpus = "0.00001"; };
assert !(controllerResourcesAccepted { memory = "99999999999999999999999999999999t"; });
assert !(controllerResourcesAccepted { cpus = "99999999999999999999999999999999"; });
assert !brokerAggregateBudgetAttempt.success;
assert brokerModeBudgetsAccepted [ llmMode ] 4 30;
assert brokerModeBudgetsAccepted [ fetchMode ] 4 30;
assert !(brokerModeBudgetsAccepted [ llmMode ] 3 30);
assert !(brokerModeBudgetsAccepted [ fetchMode ] 4 29);
assert !(brokerModeBudgetsAccepted [ dualMode ] 4 60);
assert !(brokerModeBudgetsAccepted [ dualMode ] 8 30);
assert brokerModeBudgetsAccepted [ dualMode ] 8 60;
assert brokerModeBudgetsAccepted [
  dualMode
  (dualMode // { enable = false; })
] 8 60;
assert brokerModeBudgetsAccepted [ llmMode fetchMode dualMode ] 16 120;
assert !(brokerModeBudgetsAccepted [ llmMode fetchMode dualMode ] 15 120);
assert !(brokerModeBudgetsAccepted [ llmMode fetchMode dualMode ] 16 119);

assert workerAttempt.success;
assert
  !(builtins.tryEval tooSmallQuota.config.tentaflake.workspaceQuota.agents.hermes-worker.sizeMiB)
  .success;
assert !incompleteAutostartAttempt.success;
assert !unsafeWorkerAttempt.success;
assert !unsafeQuotaAttempt.success;
assert !unsafeBrokerAttempt.success;
assert backupAttempt.success;
assert !unsafeBackupAttempt.success;
assert backup.config.services.restic.backups.tentaflake.runCheck;
assert backup.config.services.restic.backups.tentaflake.inhibitsSleep;
assert
  backup.config.services.restic.backups.tentaflake.paths == [
    "/var/lib/hermes-fixture/"
    "/var/lib/hermes-fixture/workspace"
  ];
assert
  explicitWorkspaceBackup.config.services.restic.backups.tentaflake.paths == [
    "/var/lib/hermes-fixture/workspace"
  ];
assert unrelatedBackup.config.services.restic.backups.tentaflake.paths == [ "/var/lib/unrelated" ];
assert backup.config.services.restic.backups.tentaflake.extraBackupArgs == [ "--one-file-system" ];
assert
  backup.config.systemd.services.restic-backups-tentaflake.unitConfig.RequiresMountsFor == [
    "/var/lib/hermes-fixture/"
    "/var/lib/hermes-fixture/workspace"
  ];
assert
  backup.config.systemd.services.restic-backups-tentaflake.unitConfig.AssertPathIsMountPoint == [
    "/var/lib/hermes-fixture/workspace"
  ];
assert
  unrelatedBackup.config.systemd.services.restic-backups-tentaflake.unitConfig.AssertPathIsMountPoint
  == [ ];
assert
  backup.config.systemd.services.restic-backups-tentaflake.unitConfig.OnSuccess == [
    "tentaflake-backup-success.service"
  ];
assert
  backup.config.systemd.services.tentaflake-backup-success.serviceConfig.StateDirectory
  == "tentaflake-backup";
assert
  backup.config.systemd.services.tentaflake-backup-success.serviceConfig.StateDirectoryMode == "0750";
assert lib.hasInfix "$STATE_DIRECTORY/last-success"
  backup.config.systemd.services.tentaflake-backup-success.script;
assert brokerContainer.networks == [ "tf-hermes-brokered" ];
assert
  brokerContainer.environmentFiles == [
    "/run/tentaflake-broker/hermes-brokered/agent.env"
  ];
assert !(lib.elem "--network=none" brokerContainer.extraOptions);
assert lib.elem "--dns=127.0.0.1" brokerContainer.extraOptions;
assert lib.elem "--sysctl=net.ipv6.conf.all.disable_ipv6=1" brokerContainer.extraOptions;
assert brokerContainer.labels."io.tentaflake.brokered-egress" == "true";
assert lib.elem "/var/lib/tentaflake-worker-hermes-worker/results:/run/tentaflake-worker/results:ro"
  workerContainer.volumes;
assert workerService.serviceConfig.RestrictAddressFamilies == [ "AF_UNIX" ];
assert workerService.serviceConfig.CapabilityBoundingSet == [ "CAP_DAC_READ_SEARCH" ];
assert workerService.serviceConfig.Group == "tfw-gid-10000";
assert workerService.startLimitIntervalSec == 0;
assert workerCapsule.config.users.groups.tfw-gid-10000.gid == 10000;
assert numericWorkerService.serviceConfig.Group == "nogroup";
assert
  workerPath.pathConfig.DirectoryNotEmpty
  == "/var/lib/hermes-worker/workspace/.tentaflake-worker/inbox";
assert workerPath.unitConfig.DefaultDependencies == false;
assert lib.elem "tentaflake-workspace-quota-hermes-worker.service" workerPath.after;
assert workerMount != null;
assert workerMount.what == "/var/lib/tentaflake-workspace-volumes/hermes-worker.img";
assert workerMount.options == "loop,nodev,nosuid,noatime";
assert workerMount.unitConfig.DefaultDependencies == false;
assert workerMount.wantedBy == [ "multi-user.target" ];
assert lib.elem "local-fs.target" workerMount.after;
assert workerOwnerService.script == "exec ${workerQuotaHelpers.owner}";
assert
  workerOwnerService.serviceConfig.CapabilityBoundingSet == [
    "CAP_CHOWN"
    "CAP_DAC_OVERRIDE"
    "CAP_FOWNER"
  ];
assert lib.elem
  "d /var/lib/hermes-worker/workspace/.tentaflake-worker/inbox 0770 10000 tfw-gid-10000 -"
  workerCapsule.config.systemd.tmpfiles.rules;
assert lib.elem "d /var/lib/tentaflake-worker-hermes-worker/running 0700 root root -"
  workerCapsule.config.systemd.tmpfiles.rules;
assert lib.elem "f /var/lib/tentaflake-worker-hermes-worker/.queue.lock 0600 root root -"
  workerCapsule.config.systemd.tmpfiles.rules;
assert lib.elem "tentaflake-workspace-quota-hermes-worker.service"
  workerCapsule.config.systemd.services.docker-hermes-worker.requires;
assert lib.elem "  exec ${workerQuotaHelpers.create}" workerPrepareLines;
assert lib.elem "  exec ${workerQuotaHelpers.verify}" workerPrepareLines;
assert
  workerService.serviceConfig.ReadWritePaths == [
    "/var/lib/tentaflake-worker-hermes-worker"
    "/var/lib/hermes-worker/workspace/.tentaflake-worker/inbox"
  ];
assert
  guardedWorkerService.serviceConfig.ReadWritePaths == workerService.serviceConfig.ReadWritePaths
  ++ [
    "/var/lib/tentaflake-admission.lock"
    "/var/lib/tentaflake-admission/.lock"
  ];
assert guardedWorkerService.serviceConfig.ProtectSystem == "strict";
assert guardedWorkerService.serviceConfig.ReadOnlyPaths == [ "/var/lib/hermes-worker/workspace" ];
assert guardedWorkerPlan.workers == guardedWorker.config.tentaflake.worker.maintenance.workers;
assert guardedWorkerPlan.workerImage == guardedWorker.config.tentaflake.worker.maintenance.image;
assert import ./api-policy.nix { inherit pkgs; };
assert
  builtins.attrNames guardedWorkerSpec == [
    "config"
    "executable"
    "inbox"
    "stateDir"
    "unit"
    "watcher"
  ];
assert
  guardedWorkerSpec.config
  == toString guardedWorker.config.environment.etc."tentaflake/workers/hermes-worker.json".source;
assert guardedWorkerSpec.executable == lib.getExe guardedWorker.config.tentaflake.worker.package;
assert guardedWorkerSpec.unit == "tentaflake-worker-hermes-worker.service";
assert guardedWorkerSpec.watcher == "tentaflake-worker-hermes-worker.path";
assert guardedWorkerSpec.inbox == "/var/lib/hermes-worker/workspace/.tentaflake-worker/inbox";
assert guardedWorkerSpec.stateDir == "/var/lib/tentaflake-worker-hermes-worker";
assert
  guardedWorkerService.serviceConfig.ExecStart
  == "${guardedWorkerSpec.executable} --config ${guardedWorkerSpec.config} drain";
assert
  guardedWorker.config.systemd.services.tentaflake-worker-image.serviceConfig.ExecStart
  == "${guardedWorkerPlan.tools.cli} admission worker-load ${guardedWorkerPlan.workerImage.config}";
assert
  workerCapsule.config.systemd.services.tentaflake-worker-image.serviceConfig.ExecStart
  == "${guardedWorkerPlan.tools.cli} admission worker-load ${legacyWorkerInputs.image.config}";
assert lib.all (
  fixture:
  lib.any (
    item:
    !item.assertion
    &&
      item.message
      == "tentaflake worker names must be lowercase safe OCI identifiers up to 48 characters; image and state-volumes are reserved."
  ) fixture.config.assertions
) reservedWorkerNames;
assert lib.hasInfix "host\tbalanced\tfalse\ttrue\ttrue\ttrue\tfalse\t36\n" brokerManifest;
assert lib.hasInfix "management\ttailscale\ttrue\ttrue\ttailnet-policy" brokerManifest;
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-llm-hermes-brokered".serviceConfig.LoadCredential
  == [
    "agent-token:/run/tentaflake-broker/hermes-brokered/agent-token"
    "provider:/run/agenix/example-provider"
  ];
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-fetch-hermes-brokered".serviceConfig.CapabilityBoundingSet
  == "";
assert lib.hasInfix "ip saddr 10.203.20.0/30 counter drop"
  brokerCapsule.config.networking.firewall.extraForwardRules;
assert lib.hasInfix "iifname \"${brokerBridge}\" ip saddr 10.203.20.0/30"
  brokerCapsule.config.networking.firewall.extraInputRules;
assert brokerCapsule.options.tentaflake.broker.admissionModels.readOnly;
assert brokerCapsule.options.tentaflake.broker.admissionModels.internal;
assert
  builtins.attrNames brokerProjection.tentaflake.broker.admissionModels == [ "hermes-brokered" ];
assert podmanCapsule.config.tentaflake.broker.admissionModels == { };
assert core.config.tentaflake.broker.admissionModels == { };
assert
  builtins.attrNames brokerModel == [
    "configuration"
    "environmentFile"
    "executable"
    "firewallPorts"
    "firewallTable"
    "network"
    "port"
    "schemaVersion"
    "unit"
  ];
assert brokerModel.schemaVersion == 1;
assert brokerModel.unit == "tentaflake-broker-llm-hermes-brokered.service";
assert brokerModel.executable == lib.getExe brokerProjection.tentaflake.broker.package;
assert brokerModel.environmentFile == builtins.head brokerContainer.environmentFiles;
assert brokerModel.port == 7910;
assert
  brokerModel.firewallPorts == [
    7810
    7910
  ];
assert
  brokerModel.network == {
    name = "tf-hermes-brokered";
    bridge = brokerBridge;
    subnet = "10.203.20.0/30";
    gateway = "10.203.20.1";
  };
assert builtins.hasContext brokerModel.configuration;
assert
  brokerProjection.systemd.services.tentaflake-broker-llm-hermes-brokered.serviceConfig.ExecStart
  == "${brokerModel.executable} --config ${brokerModel.configuration}";
assert !(lib.hasInfix "/run/agenix/example-provider" (builtins.toJSON brokerModel));
assert
  lib.filter (lib.hasPrefix "tentaflake-broker-") (
    builtins.attrNames brokerProjection.networking.nftables.tables
  ) == [ brokerModel.firewallTable ];
assert brokerBoundary.enable;
assert brokerBoundary.family == "inet";
assert brokerBoundary.name == brokerModel.firewallTable;
assert lib.elem 2222 brokerProjection.networking.firewall.allowedTCPPorts;
# This exact standalone table must retain its own hooks and terminal bridge
# drops even when NixOS accepts a configured host port or DNAT earlier.
assert
  brokerBoundary.content == ''
    chain input {
      type filter hook input priority -10; policy accept;
      iifname "${brokerBridge}" ip saddr 10.203.20.0/30 ip daddr 10.203.20.1 tcp dport { 7810, 7910 } accept
      iifname "${brokerBridge}" drop
    }
    chain forward {
      type filter hook forward priority -10; policy accept;
      iifname "${brokerBridge}" drop
    }
  '';
assert
  let
    tables = lib.filterAttrs (
      name: _: lib.hasPrefix "tentaflake-broker-" name
    ) podmanCapsule.config.networking.nftables.tables;
  in
  builtins.length (builtins.attrNames tables) == 1
  && lib.hasInfix "tcp dport { 7811 } accept" (builtins.head (builtins.attrValues tables)).content;
assert lib.hasInfix "com.docker.network.bridge.name=\"$bridge\""
  brokerCapsule.config.systemd.services.tentaflake-broker-network-hermes-brokered.script;
assert lib.all
  (
    container:
    container.ports == [ ]
    && container.privileged == false
    && container.capabilities.ALL == false
    && lib.elem "--network=none" (capsuleFlags container)
    && lib.elem "--read-only" (capsuleFlags container)
    && lib.elem "--runtime=runsc" (capsuleFlags container)
    && lib.elem "--security-opt=no-new-privileges:true" (capsuleFlags container)
    && lib.elem "--security-opt=apparmor=docker-default" (capsuleFlags container)
    && lib.elem "--memory=2g" (capsuleFlags container)
    && lib.elem "--memory-swap=2g" (capsuleFlags container)
    && lib.elem "--cpus=2.0" (capsuleFlags container)
    && lib.elem "--pids-limit=512" (capsuleFlags container)
  )
  [
    hermesCapsule
    zeroclawCapsule
  ];
assert hermesCapsule.user == "10000:10000";
assert zeroclawCapsule.user == "65534:65534";
assert observability.config.services.prometheus.listenAddress == "127.0.0.1";
assert observability.config.services.prometheus.exporters.node.listenAddress == "127.0.0.1";
assert observability.config.services.grafana.settings.server.http_addr == "127.0.0.1";
assert observability.config.services.loki.configuration.server.http_listen_address == "127.0.0.1";
assert observability.config.services.alloy.enable;
assert lib.elem "textfile"
  observability.config.services.prometheus.exporters.node.enabledCollectors;
assert lib.any (lib.hasInfix "TentaflakePolicyDenials")
  observability.config.services.prometheus.rules;
assert lib.hasInfix "/var/lib/tentaflake-broker-llm-hermes-observed/audit.jsonl"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert
  observability.config.systemd.services.tentaflake-observability-metrics.serviceConfig.RestrictAddressFamilies
  == [ "AF_UNIX" ];
assert lib.elem "--server.http.listen-addr=127.0.0.1:12346"
  observability.config.services.alloy.extraFlags;
assert observability.config.networking.firewall.allowedTCPPorts == [ ];
assert
  observability.config.systemd.services.grafana.serviceConfig.LoadCredential == [
    "secret-key:/run/credentials/grafana-secret-key"
    "admin-password:/run/credentials/grafana-admin-password"
  ];
assert lib.hasInfix "engine.kind=modern_ebpf"
  falco.config.systemd.services.tentaflake-falco.serviceConfig.ExecStart;
assert
  falco.config.systemd.services.tentaflake-falco.serviceConfig.CapabilityBoundingSet == [
    "CAP_BPF"
    "CAP_PERFMON"
    "CAP_SYS_RESOURCE"
    "CAP_SYS_PTRACE"
  ];
assert
  !(lib.elem "CAP_SYS_ADMIN" falco.config.systemd.services.tentaflake-falco.serviceConfig.CapabilityBoundingSet);
assert
  falco.config.systemd.services.tentaflake-falco.serviceConfig.RestrictAddressFamilies == [
    "AF_UNIX"
    "AF_NETLINK"
  ];
# Manual enrollment still applies SSH/hostname preferences. Tags belong to up,
# including automatic key enrollment; they are not accepted by tailscale set.
assert capsule.config.services.tailscale.authKeyFile == null;
assert
  capsule.config.services.tailscale.extraSetFlags == [
    "--hostname=eval-host"
    "--ssh"
  ];
assert
  capsule.config.services.tailscale.extraUpFlags == [
    "--advertise-tags=tag:agent-host"
    "--hostname=eval-host"
    "--ssh"
  ];
assert builtins.hasAttr "tailscaled-set" capsule.config.systemd.services;
assert lib.hasInfix " set " capsule.config.systemd.services.tailscaled-set.script;
assert !(builtins.hasAttr "tailscaled-autoconnect" capsule.config.systemd.services);
assert
  authKeyHost.config.services.tailscale.extraSetFlags
  == capsule.config.services.tailscale.extraSetFlags;
assert
  authKeyHost.config.services.tailscale.extraUpFlags
  == capsule.config.services.tailscale.extraUpFlags;
assert lib.hasInfix "tailscale up --auth-key"
  authKeyHost.config.systemd.services.tailscaled-autoconnect.script;
assert lib.hasInfix "--advertise-tags=tag:agent-host"
  authKeyHost.config.systemd.services.tailscaled-autoconnect.script;
assert builtins.length tailscalePolicy.grants == 1;
assert builtins.length tailscalePolicy.ssh == 1;
assert (builtins.head tailscalePolicy.ssh).action == "check";
# Parse the configured flags with the pinned CLI. --help avoids contacting a
# daemon, while an unsupported flag before it still fails during parsing.
pkgs.runCommand "tentaflake-module-evaluation" { } ''
  # Read the actual generated JSON artifacts without introducing IFD.
  ${lib.getExe pkgs.jq} -e --arg cli '${guardedWorkerPlan.tools.cli}' \
    '.admission_cli == $cli and .agent == "hermes-worker"' ${guardedWorkerSpec.config} >/dev/null
  ${lib.getExe pkgs.jq} -e '.admission_cli == null' ${legacyWorkerInputs.workers.hermes-worker.config} >/dev/null
  ${lib.getExe pkgs.jq} -e --arg runtime '${lib.getExe guardedWorker.config.virtualisation.docker.package}' \
    --arg archive '${guardedWorker.config.tentaflake.worker.image}' \
    '.schemaVersion == 1 and .enforced == true and .backend == "docker" and .runtime == $runtime and .archive == $archive' \
    ${guardedWorkerPlan.workerImage.config} >/dev/null
  ${lib.getExe pkgs.jq} -e --arg runtime '${lib.getExe workerCapsule.config.virtualisation.docker.package}' \
    --arg archive '${workerCapsule.config.tentaflake.worker.image}' \
    '.schemaVersion == 1 and .enforced == false and .backend == "docker" and .runtime == $runtime and .archive == $archive' \
    ${legacyWorkerInputs.image.config} >/dev/null
  ${lib.getExe pkgs.jq} -e --arg runtime '${lib.getExe podmanCapsule.config.virtualisation.podman.package}' \
    '.schemaVersion == 1 and .enforced == false and .backend == "podman" and .runtime == $runtime' \
    ${podmanWorkerInputs.image.config} >/dev/null
  # Quota services delegate to immutable helpers. Check the selected artifacts
  # here, without importing derivation contents during module evaluation.
  grep -Fq 'refusing unsafe worker control path' ${workerQuotaHelpers.owner}
  grep -Fq 'refusing to hide non-empty workspace' ${workerQuotaHelpers.create}
  grep -Fq 'refusing to hide non-empty workspace' ${workerQuotaHelpers.verify}
  ${lib.getExe capsule.config.services.tailscale.package} set ${lib.escapeShellArgs capsule.config.services.tailscale.extraSetFlags} --help >/dev/null
  ${lib.getExe capsule.config.services.tailscale.package} up ${lib.escapeShellArgs capsule.config.services.tailscale.extraUpFlags} --help >/dev/null
  touch $out
''
