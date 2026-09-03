{ nixpkgsPath }:
let
  system = "x86_64-linux";
  pkgs = import nixpkgsPath { inherit system; };
  inherit (pkgs) lib;
  evalConfig = import (nixpkgsPath + "/nixos/lib/eval-config.nix");
  eval =
    modules:
    evalConfig {
      inherit system modules;
      specialArgs = { };
    };
  builders = import ../lib { inherit pkgs lib; };
  containerSecurity = import ../lib/containerSecurity.nix { inherit lib; };
  hostModule = profile: {
    system.stateVersion = "26.05";
    fileSystems."/" = {
      device = "/dev/disk/by-label/nixos";
      fsType = "ext4";
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
  capsuleLiveResources = builtins.toJSON {
    memoryBytes = 2147483648;
    memorySwapBytes = 2147483648;
    nanoCpus = 2000000000;
    pidsLimit = 512;
    nofile = 4096;
    nproc = 512;
  };
  capsuleAttempt = builtins.tryEval capsule.config.system.build.toplevel.drvPath;

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
      tentaflake.networking.enable = lib.mkForce true;
      tentaflake.broker.agents.hermes-podman-fixture = {
        enable = true;
        networkName = "tf-hermes-podman";
        subnet = "10.203.40.0/30";
        gateway = "10.203.40.1";
        fetch = {
          enable = true;
          allowedHosts = [ "docs.example.com" ];
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
  supportedMountPolicies = map (volume: policyFixture { volumes = [ volume ]; }) [
    "/var/lib/fixture:/state"
    "/var/lib/fixture:/state:rw"
    "/var/lib/fixture:/state:ro"
  ];
  unsafeWritableMountOptionsPolicy = policyFixture {
    volumes = [ "/var/lib/fixture:/state:rw,rprivate" ];
  };
  unsafeReadOnlyMountOptionsPolicy = policyFixture {
    volumes = [ "/var/lib/fixture:/state:ro,Z" ];
  };
  unsafeAllowedBackingMountPolicy = containerSecurity.apply {
    profile = "balanced";
    backend = "docker";
    name = "unsafe-backing-fixture";
    owner = "10000:10000";
    baseConfig = {
      image = "example.invalid/image@sha256:fixture";
      volumes = [
        "/var/lib/tentaflake-workspace-volumes:/state:rw"
      ];
    };
    allowedWritableSources = [ "/var/lib/tentaflake-workspace-volumes" ];
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

  unsafeHermesStateFixture = eval [
    ../modules/default.nix
    (hostModule "dev")
    (builders.mkHermesAgent {
      name = "unsafe-state";
      stateDir = "/etc";
      autoStart = false;
    })
  ];
  unsafeHermesStateAttempt = builtins.tryEval unsafeHermesStateFixture.config.system.build.toplevel.drvPath;

  unsafeZeroStateFixture = eval [
    ../modules/default.nix
    (hostModule "dev")
    (builders.mkZeroClawAgent {
      name = "unsafe-state";
      stateDir = "/var/lib/tentaflake-worker-state-volumes/zeroclaw-unsafe-state";
      autoStart = false;
    })
  ];
  unsafeZeroStateAttempt = builtins.tryEval unsafeZeroStateFixture.config.system.build.toplevel.drvPath;

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
      extraVolumes = [ "/srv/dev-fixture:/fixture:ro,Z" ];
    })
    {
      tentaflake.security.resources = {
        tmpfsSize = "runtime-default";
        runTmpfsSize = "0";
      };
    }
  ];
  devContainer = devFixture.config.virtualisation.oci-containers.containers."hermes-dev";
  devManifest = devFixture.config.environment.etc."tentaflake/security.tsv".text;

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
  brokerCredentials =
    brokerCapsule.config.systemd.services."tentaflake-broker-credentials-hermes-brokered";
  brokerAttempt = builtins.tryEval brokerCapsule.config.system.build.toplevel.drvPath;

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

  brokerAgentCountFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      virtualisation.oci-containers.containers = {
        count-one = {
          image = "example.invalid/count-one@sha256:0000000000000000000000000000000000000000000000000000000000000000";
          autoStart = false;
        };
        count-two = {
          image = "example.invalid/count-two@sha256:0000000000000000000000000000000000000000000000000000000000000000";
          autoStart = false;
        };
      };
      tentaflake = {
        networking.enable = lib.mkForce true;
        broker = {
          maxEnabledAgents = 1;
          agents = {
            count-one = {
              enable = true;
              subnet = "10.203.25.0/30";
              gateway = "10.203.25.1";
              fetch = {
                enable = true;
                allowedHosts = [ "docs.example.com" ];
              };
            };
            count-two = {
              enable = true;
              subnet = "10.203.26.0/30";
              gateway = "10.203.26.1";
              fetch = {
                enable = true;
                allowedHosts = [ "docs.example.com" ];
              };
            };
          };
        };
      };
    }
  ];
  brokerAgentCountAttempt = builtins.tryEval brokerAgentCountFixture.config.system.build.toplevel.drvPath;

  workerValidationAttempt =
    name: overrides:
    let
      containerName = "validation-${name}";
      fixture = eval [
        ../modules/default.nix
        (hostModule "balanced")
        {
          virtualisation.oci-containers.containers.${containerName} = {
            image = "example.invalid/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000";
            autoStart = false;
          };
          tentaflake.worker.agents.${containerName} = {
            enable = true;
            workspace = "/var/lib/${containerName}/workspace";
          }
          // overrides;
        }
      ];
    in
    builtins.tryEval fixture.config.system.build.toplevel.drvPath;
  unsafeDotWorkerAttempt = workerValidationAttempt "dot-path" {
    workspace = "/var/lib/validation-dot-path/./workspace";
  };
  unsafeBackingWorkerAttempt = workerValidationAttempt "backing-path" {
    workspace = "/var/lib/tentaflake-worker-state-volumes/validation-backing-path";
  };
  rootWorkerAttempt = workerValidationAttempt "root-id" {
    containerUid = 0;
  };
  invalidWorkerMemoryAttempt = workerValidationAttempt "memory-zero" {
    memory = "0";
  };
  invalidWorkerMemoryBoundAttempt = workerValidationAttempt "memory-large" {
    memory = "2t";
  };
  invalidWorkerSwapAttempt = workerValidationAttempt "swap-small" {
    memory = "64m";
    memorySwap = "32m";
  };
  invalidWorkerCpusAttempt = workerValidationAttempt "cpus-zero" {
    cpus = "0";
  };
  invalidWorkerCpusBoundAttempt = workerValidationAttempt "cpus-large" {
    cpus = "1025";
  };
  invalidWorkerCpusPrecisionAttempt = workerValidationAttempt "cpus-overprecise" {
    cpus = "0.000001";
  };
  invalidWorkerTmpfsAttempt = workerValidationAttempt "tmpfs-zero" {
    workspaceTmpfsSize = "0";
  };
  invalidWorkerTmpfsBoundAttempt = workerValidationAttempt "tmpfs-large" {
    workspaceTmpfsSize = "65g";
  };
  invalidWorkerTimeoutAttempt = workerValidationAttempt "timeout-large" {
    maxTimeoutSeconds = 86401;
  };
  invalidWorkerPidsAttempt = workerValidationAttempt "pids-large" {
    pidsLimit = 65537;
  };
  invalidWorkerSnapshotBytesAttempt = workerValidationAttempt "snapshot-bytes-large" {
    maxSnapshotBytes = 16 * 1024 * 1024 * 1024 + 1;
  };
  invalidWorkerSnapshotEntriesAttempt = workerValidationAttempt "snapshot-entries-large" {
    maxSnapshotEntries = 2000001;
  };
  invalidWorkerLogBytesAttempt = workerValidationAttempt "log-bytes-large" {
    maxLogBytes = 64 * 1024 * 1024 + 1;
  };

  workerAgentCountFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      virtualisation.oci-containers.containers = {
        worker-count-one = {
          image = "example.invalid/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000";
          autoStart = false;
        };
        worker-count-two = {
          image = "example.invalid/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000";
          autoStart = false;
        };
      };
      tentaflake.worker = {
        maxEnabledAgents = 1;
        agents = {
          worker-count-one = {
            enable = true;
            workspace = "/var/lib/worker-count-one/workspace";
          };
          worker-count-two = {
            enable = true;
            workspace = "/var/lib/worker-count-two/workspace";
          };
        };
      };
    }
  ];
  workerAgentCountAttempt = builtins.tryEval workerAgentCountFixture.config.system.build.toplevel.drvPath;

  workerStateBudgetFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      virtualisation.oci-containers.containers.state-budget = {
        image = "example.invalid/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000";
        autoStart = false;
      };
      tentaflake.worker = {
        maxTotalStateVolumeMiB = 8191;
        agents.state-budget = {
          enable = true;
          workspace = "/var/lib/state-budget/workspace";
        };
      };
    }
  ];
  workerStateBudgetAttempt = builtins.tryEval workerStateBudgetFixture.config.system.build.toplevel.drvPath;

  workerMemoryBudgetFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      virtualisation.oci-containers.containers.memory-budget = {
        image = "example.invalid/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000";
        autoStart = false;
      };
      tentaflake.worker = {
        maxTotalMemoryBytes = 2147483647;
        agents.memory-budget = {
          enable = true;
          workspace = "/var/lib/memory-budget/workspace";
        };
      };
    }
  ];
  workerMemoryBudgetAttempt = builtins.tryEval workerMemoryBudgetFixture.config.system.build.toplevel.drvPath;

  workerPidBudgetFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      virtualisation.oci-containers.containers.pid-budget = {
        image = "example.invalid/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000";
        autoStart = false;
      };
      tentaflake.worker = {
        maxTotalPids = 511;
        agents.pid-budget = {
          enable = true;
          workspace = "/var/lib/pid-budget/workspace";
        };
      };
    }
  ];
  workerPidBudgetAttempt = builtins.tryEval workerPidBudgetFixture.config.system.build.toplevel.drvPath;

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
      tentaflake = {
        networking.enable = lib.mkForce true;
        broker.agents.hermes-worker = {
          enable = true;
          networkName = "tf-hermes-worker";
          subnet = "10.203.22.0/30";
          gateway = "10.203.22.1";
          fetch = {
            enable = true;
            allowedHosts = [ "docs.example.com" ];
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
          sizeMiB = 32;
        };
      };
    }
  ];
  workerContainer = workerCapsule.config.virtualisation.oci-containers.containers.hermes-worker;
  workerService = workerCapsule.config.systemd.services.tentaflake-worker-hermes-worker;
  numericWorkerService = workerCapsule.config.systemd.services.tentaflake-worker-zeroclaw-worker;
  workerPath = workerCapsule.config.systemd.paths.tentaflake-worker-hermes-worker;
  workerOwnerService = workerCapsule.config.systemd.services.tentaflake-workspace-quota-hermes-worker;
  workerStateService = workerCapsule.config.systemd.services.tentaflake-worker-state-hermes-worker;
  workerStateCleanup =
    workerCapsule.config.systemd.services.tentaflake-worker-result-cleanup-hermes-worker;
  workerStateCleanupTimer =
    workerCapsule.config.systemd.timers.tentaflake-worker-result-cleanup-hermes-worker;
  workerAttempt = builtins.tryEval workerCapsule.config.system.build.toplevel.drvPath;
  workerMount = lib.findFirst (
    mount: mount.where == "/var/lib/hermes-worker/workspace"
  ) null workerCapsule.config.systemd.mounts;
  workerStateMount = lib.findFirst (
    mount: mount.where == "/var/lib/tentaflake-worker-hermes-worker"
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

  unsafeQuotaDotFixture = eval [
    ../modules/default.nix
    (hostModule "balanced")
    (builders.mkHermesAgent {
      name = "unsafe-quota-dot";
      autoStart = false;
    })
    {
      tentaflake.workspaceQuota.agents.hermes-unsafe-quota-dot = {
        enable = true;
        workspace = "/var/lib/hermes-unsafe-quota-dot/./workspace";
      };
    }
  ];
  unsafeQuotaDotAttempt = builtins.tryEval unsafeQuotaDotFixture.config.system.build.toplevel.drvPath;

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

  backup = eval [
    ../modules/default.nix
    (hostModule "balanced")
    {
      tentaflake.backup = {
        enable = true;
        paths = [ "/var/lib/hermes-fixture" ];
        repositoryFile = "/run/credentials/restic-repository";
        passwordFile = "/run/credentials/restic-password";
      };
    }
  ];
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
        worker.agents.hermes-observed = {
          enable = true;
          workspace = "/var/lib/hermes-observed/workspace";
        };
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
in
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
assert lib.hasInfix "host\tbalanced\tfalse\tfalse\ttrue\ttrue\ttrue" capsuleManifest;
assert lib.hasInfix "agent\thermes-hermes-fixture\tbalanced" capsuleManifest;
assert lib.hasInfix "\ttrue\t${capsuleLiveResources}\n" capsuleManifest;
assert
  lib.getExe' pkgs.gvisor "runsc"
  == capsule.config.virtualisation.docker.daemon.settings.runtimes.runsc.path;
assert
  podmanContainer.preRunExtraOptions == [
    "--runtime"
    "runsc"
  ];
assert lib.elem pkgs.gvisor podmanCapsule.config.virtualisation.podman.extraRuntimes;
assert podmanContainer.networks == [ "tf-hermes-podman" ];
assert lib.hasInfix "--interface-name \"$bridge\""
  podmanCapsule.config.systemd.services.tentaflake-broker-network-hermes-podman-fixture.script;
assert !(lib.all (item: item.assertion) unsafePolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeEnvironmentPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeOptionPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeGpuPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeMountPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeSharedMountPolicy.assertions);
assert lib.all (policy: lib.all (item: item.assertion) policy.assertions) supportedMountPolicies;
assert !(lib.all (item: item.assertion) unsafeWritableMountOptionsPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeReadOnlyMountOptionsPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeAllowedBackingMountPolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeImageArchivePolicy.assertions);
assert !(lib.all (item: item.assertion) unsafeDevNamePolicy.assertions);
assert !mutableImageAttempt.success;
assert !publishedPortAttempt.success;
assert !rootHermesAttempt.success;
assert !unsafeHermesStateAttempt.success;
assert !unsafeZeroStateAttempt.success;
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
assert lib.hasInfix "\tfalse\t-\t-\t-\t-\t-\t-\n" devManifest;
assert brokerAttempt.success;
assert !brokerAggregateBudgetAttempt.success;
assert !brokerAgentCountAttempt.success;
assert workerAttempt.success;
assert !unsafeDotWorkerAttempt.success;
assert !unsafeBackingWorkerAttempt.success;
assert !rootWorkerAttempt.success;
assert !invalidWorkerMemoryAttempt.success;
assert !invalidWorkerMemoryBoundAttempt.success;
assert !invalidWorkerSwapAttempt.success;
assert !invalidWorkerCpusAttempt.success;
assert !invalidWorkerCpusBoundAttempt.success;
assert !invalidWorkerCpusPrecisionAttempt.success;
assert !invalidWorkerTmpfsAttempt.success;
assert !invalidWorkerTmpfsBoundAttempt.success;
assert !invalidWorkerTimeoutAttempt.success;
assert !invalidWorkerPidsAttempt.success;
assert !invalidWorkerSnapshotBytesAttempt.success;
assert !invalidWorkerSnapshotEntriesAttempt.success;
assert !invalidWorkerLogBytesAttempt.success;
assert !workerAgentCountAttempt.success;
assert !workerStateBudgetAttempt.success;
assert !workerMemoryBudgetAttempt.success;
assert !workerPidBudgetAttempt.success;
assert !incompleteAutostartAttempt.success;
assert !unsafeWorkerAttempt.success;
assert !unsafeQuotaAttempt.success;
assert !unsafeQuotaDotAttempt.success;
assert !unsafeBrokerAttempt.success;
assert backupAttempt.success;
assert !unsafeBackupAttempt.success;
assert backup.config.services.restic.backups.tentaflake.runCheck;
assert backup.config.services.restic.backups.tentaflake.inhibitsSleep;
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
assert backup.config.systemd.services.tentaflake-backup-success.serviceConfig.ProtectClock;
assert backup.config.systemd.services.tentaflake-backup-success.serviceConfig.ProtectControlGroups;
assert backup.config.systemd.services.tentaflake-backup-success.serviceConfig.RestrictNamespaces;
assert backup.config.systemd.services.tentaflake-backup-success.serviceConfig.RestrictSUIDSGID;
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
assert workerService.serviceConfig.TimeoutStartSec == "infinity";
assert workerService.serviceConfig.MemoryMax == 256 * 1024 * 1024;
assert workerService.serviceConfig.TasksMax == 64;
assert workerService.serviceConfig.CPUQuota == "50%";
assert workerService.serviceConfig.LimitNOFILE == 4096;
assert workerService.serviceConfig.Nice == 10;
assert workerService.startLimitIntervalSec == 0;
assert workerCapsule.config.users.groups.tfw-gid-10000.gid == 10000;
assert lib.elem "tentaflake-worker-state-hermes-worker.service" workerService.requires;
assert workerService.bindsTo == [ "tentaflake-worker-state-hermes-worker.service" ];
assert lib.elem "tentaflake-worker-state-hermes-worker.service"
  workerCapsule.config.systemd.services.docker-hermes-worker.requires;
assert
  workerCapsule.config.systemd.services.docker-hermes-worker.bindsTo
  == [ "tentaflake-worker-state-hermes-worker.service" ];
assert workerStateService.serviceConfig.RemainAfterExit;
assert
  workerStateService.serviceConfig.CapabilityBoundingSet == [
    "CAP_CHOWN"
    "CAP_DAC_OVERRIDE"
    "CAP_FOWNER"
    "CAP_FSETID"
  ];
assert builtins.length workerStateService.bindsTo == 1;
assert
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.serviceConfig.MemoryMax
  == 256 * 1024 * 1024;
assert
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.serviceConfig.TasksMax
  == 32;
assert
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.serviceConfig.CPUQuota
  == "25%";
assert
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.serviceConfig.Nice
  == 10;
assert lib.hasInfix "refusing to hide non-empty worker state"
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.script;
assert lib.hasInfix "fallocate --length \"$expected\""
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.script;
assert lib.hasInfix "cannot fully preallocate worker state image"
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.script;
assert lib.hasInfix "mkfs.ext4 -F -q -m 0 -i 16384"
  workerCapsule.config.systemd.services.tentaflake-worker-state-prepare-hermes-worker.script;
assert lib.hasInfix ".tentaflake-worker-state-v1" workerStateService.script;
assert lib.hasInfix "find \"$results\" -xdev -mindepth 1 -maxdepth 1 -type d -mtime +13 -exec"
  workerStateCleanup.script;
assert lib.hasInfix "find -- {} -xdev -depth -ignore_readdir_race -delete \\;"
  workerStateCleanup.script;
assert workerStateCleanup.serviceConfig.TimeoutStartSec == "5min";
assert workerStateCleanup.serviceConfig.MemoryMax == 128 * 1024 * 1024;
assert workerStateCleanup.serviceConfig.TasksMax == 32;
assert workerStateCleanup.serviceConfig.CPUQuota == "25%";
assert workerStateCleanup.serviceConfig.Nice == 10;
assert workerStateCleanup.serviceConfig.RestrictAddressFamilies == [ "AF_UNIX" ];
assert workerStateCleanup.serviceConfig.RestrictNamespaces;
assert workerStateCleanup.serviceConfig.RestrictRealtime;
assert workerStateCleanup.serviceConfig.RestrictSUIDSGID;
assert workerStateCleanup.serviceConfig.LockPersonality;
assert workerStateCleanup.serviceConfig.SystemCallArchitectures == "native";
assert workerStateCleanupTimer.requires == [ "tentaflake-worker-state-hermes-worker.service" ];
assert workerStateCleanupTimer.after == [ "tentaflake-worker-state-hermes-worker.service" ];
assert workerStateCleanupTimer.bindsTo == [ "tentaflake-worker-state-hermes-worker.service" ];
assert workerStateCleanupTimer.timerConfig.RandomizedDelaySec == "6h";
assert workerStateCleanupTimer.timerConfig.FixedRandomDelay;
assert numericWorkerService.serviceConfig.Group == "nogroup";
assert
  workerPath.pathConfig.PathChanged == "/var/lib/hermes-worker/workspace/.tentaflake-worker/inbox";
assert !(workerPath.pathConfig ? DirectoryNotEmpty);
assert workerPath.bindsTo == [ "tentaflake-worker-state-hermes-worker.service" ];
assert workerPath.unitConfig.DefaultDependencies == false;
assert lib.elem "tentaflake-workspace-quota-hermes-worker.service" workerPath.after;
assert workerMount != null;
assert workerMount.what == "/var/lib/tentaflake-workspace-volumes/hermes-worker.img";
assert workerMount.options == "loop,nodev,nosuid,noatime";
assert workerMount.unitConfig.DefaultDependencies == false;
assert workerMount.wantedBy == [ "multi-user.target" ];
assert lib.elem "local-fs.target" workerMount.after;
assert workerStateMount != null;
assert workerStateMount.what == "/var/lib/tentaflake-worker-state-volumes/hermes-worker.img";
assert workerStateMount.where == "/var/lib/tentaflake-worker-hermes-worker";
assert workerStateMount.options == "loop,nodev,nosuid,noexec,noatime";
assert workerStateMount.unitConfig.DefaultDependencies == false;
assert workerStateMount.wantedBy == [ "multi-user.target" ];
assert lib.elem "local-fs.target" workerStateMount.after;
assert lib.hasInfix "refusing unsafe worker control path" workerOwnerService.script;
assert
  workerOwnerService.serviceConfig.CapabilityBoundingSet == [
    "CAP_CHOWN"
    "CAP_DAC_OVERRIDE"
    "CAP_FOWNER"
  ];
assert lib.elem "d /var/lib/tentaflake-worker-hermes-worker 0750 root root -"
  workerCapsule.config.systemd.tmpfiles.rules;
assert lib.elem
  "d /var/lib/hermes-worker/workspace/.tentaflake-worker/inbox 0770 10000 tfw-gid-10000 -"
  workerCapsule.config.systemd.tmpfiles.rules;
assert lib.elem "tentaflake-workspace-quota-hermes-worker.service"
  workerCapsule.config.systemd.services.docker-hermes-worker.requires;
assert lib.hasInfix "refusing to hide non-empty workspace"
  workerCapsule.config.systemd.services.tentaflake-workspace-quota-prepare-hermes-worker.script;
assert lib.hasInfix "fallocate --length \"$expected\""
  workerCapsule.config.systemd.services.tentaflake-workspace-quota-prepare-hermes-worker.script;
assert lib.hasInfix "cannot fully preallocate workspace image"
  workerCapsule.config.systemd.services.tentaflake-workspace-quota-prepare-hermes-worker.script;
assert lib.hasInfix " load --input "
  workerCapsule.config.systemd.services.tentaflake-worker-image.serviceConfig.ExecStart;
assert lib.hasInfix "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue" brokerManifest;
assert lib.hasInfix (builtins.toJSON brokerContainer.volumes) brokerManifest;
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-llm-hermes-brokered".serviceConfig.LoadCredential
  == [
    "agent-token:/run/tentaflake-broker/hermes-brokered/agent-token"
    "provider:/run/agenix/example-provider"
  ];
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-fetch-hermes-brokered".serviceConfig.CapabilityBoundingSet
  == [ ];
assert brokerCredentials.serviceConfig.CapabilityBoundingSet == [ ];
assert brokerCredentials.serviceConfig.RestrictAddressFamilies == [ "AF_UNIX" ];
assert brokerCredentials.serviceConfig.RestrictNamespaces;
assert brokerCredentials.serviceConfig.RestrictRealtime;
assert brokerCredentials.serviceConfig.RestrictSUIDSGID;
assert brokerCredentials.serviceConfig.LockPersonality;
assert brokerCredentials.serviceConfig.ProtectClock;
assert brokerCredentials.serviceConfig.ProtectControlGroups;
assert brokerCredentials.serviceConfig.SystemCallArchitectures == "native";
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-fetch-hermes-brokered".serviceConfig.MemoryMax
  == 128 * 1024 * 1024;
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-fetch-hermes-brokered".serviceConfig.TasksMax
  == 64;
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-fetch-hermes-brokered".serviceConfig.LimitNOFILE
  == 4096;
assert lib.hasInfix "ip saddr 10.203.20.0/30 counter drop"
  brokerCapsule.config.networking.firewall.extraForwardRules;
assert lib.hasInfix "iifname \"${brokerBridge}\" ip saddr 10.203.20.0/30"
  brokerCapsule.config.networking.firewall.extraInputRules;
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
assert lib.hasInfix "tail -q -n 500"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "tail -n 500 |"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "jq -R -s"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "type == \"number\""
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "tentaflake_worker_enabled{agent=\"hermes-observed\"} 1\\n"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "tentaflake_worker_pending_request_limit"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "tentaflake_worker_pending_bytes_limit"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "tentaflake_worker_state_volume_bytes"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "tentaflake_worker_pids_limit"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert lib.hasInfix "tentaflake_worker_pids_limit{agent=\"hermes-observed\"} 512\\n"
  observability.config.systemd.services.tentaflake-observability-metrics.script;
assert
  observability.config.systemd.services.tentaflake-observability-metrics.serviceConfig.TimeoutStartSec
  == "45s";
assert
  observability.config.systemd.services.tentaflake-observability-metrics.serviceConfig.MemoryMax
  == 256 * 1024 * 1024;
assert
  observability.config.systemd.services.tentaflake-observability-metrics.serviceConfig.TasksMax == 32;
assert
  observability.config.systemd.services.tentaflake-observability-metrics.serviceConfig.CPUQuota
  == "50%";
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
assert builtins.length tailscalePolicy.grants == 1;
assert builtins.length tailscalePolicy.ssh == 1;
assert (builtins.head tailscalePolicy.ssh).action == "check";
true
