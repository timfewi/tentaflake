{ nixpkgsPath }:
let
  system = "x86_64-linux";
  pkgs = import nixpkgsPath { inherit system; };
  lib = pkgs.lib;
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
      tentaflake.networking.enable = lib.mkForce true;
      tentaflake.broker.agents.hermes-worker = {
        enable = true;
        networkName = "tf-hermes-worker";
        subnet = "10.203.22.0/30";
        gateway = "10.203.22.1";
        fetch = {
          enable = true;
          allowedHosts = [ "docs.example.com" ];
        };
      };
      tentaflake.worker.agents.hermes-worker = {
        enable = true;
        workspace = "/var/lib/hermes-worker/workspace";
      };
      tentaflake.worker.agents.zeroclaw-worker = {
        enable = true;
        workspace = "/var/lib/zeroclaw-worker/data";
        containerUid = 65534;
        containerGid = 65534;
      };
      tentaflake.workspaceQuota.agents.hermes-worker = {
        enable = true;
        workspace = "/var/lib/hermes-worker/workspace";
        sizeMiB = 32;
      };
    }
  ];
  workerContainer = workerCapsule.config.virtualisation.oci-containers.containers.hermes-worker;
  workerService = workerCapsule.config.systemd.services.tentaflake-worker-hermes-worker;
  numericWorkerService = workerCapsule.config.systemd.services.tentaflake-worker-zeroclaw-worker;
  workerPath = workerCapsule.config.systemd.paths.tentaflake-worker-hermes-worker;
  workerOwnerService = workerCapsule.config.systemd.services.tentaflake-workspace-quota-hermes-worker;
  workerAttempt = builtins.tryEval workerCapsule.config.system.build.toplevel.drvPath;
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
      tentaflake.profiles.observability = {
        enable = true;
        grafanaSecretKeyFile = "/run/credentials/grafana-secret-key";
        grafanaAdminPasswordFile = "/run/credentials/grafana-admin-password";
      };
      tentaflake.networking.enable = lib.mkForce true;
      tentaflake.broker.agents.hermes-observed = {
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
assert !(missingProvenanceAttempt.success);
assert lib.hasInfix "cosign verify --certificate-identity"
  provenanceService.serviceConfig.ExecStart;
assert lib.elem "tentaflake-image-verify-hermes-signed.service"
  provenanceCapsule.config.systemd.services.docker-hermes-signed.requires;
assert podmanAttempt.success;
assert !(strictAttempt.success);
assert !(unlimitedResourceAttempt.success);
assert !(missingManagementAttempt.success);
assert !(publicSshAttempt.success);
assert gitAttempt.success;
assert !(unsafeGitAttempt.success);
assert !(unsafeGitRootAttempt.success);
assert lib.hasInfix "remote-check" gitScript;
assert lib.hasInfix "safe.directory=\"$repo\"" gitScript;
assert !(lib.hasInfix "safe.directory='*'" gitScript);
assert !(lib.hasInfix "case \"$url\" in *github.com*" gitScript);
assert capsule.config.tentaflake.security.profile == "balanced";
assert lib.hasInfix "host\tbalanced\tfalse\tfalse\ttrue\ttrue\ttrue" capsuleManifest;
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
assert !(mutableImageAttempt.success);
assert !(publishedPortAttempt.success);
assert !(rootHermesAttempt.success);
assert !(unsafeHealAttempt.success);
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
assert workerAttempt.success;
assert !(incompleteAutostartAttempt.success);
assert !(unsafeWorkerAttempt.success);
assert !(unsafeQuotaAttempt.success);
assert !(unsafeBrokerAttempt.success);
assert backupAttempt.success;
assert !(unsafeBackupAttempt.success);
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
assert lib.hasInfix "refusing unsafe worker control path" workerOwnerService.script;
assert
  workerOwnerService.serviceConfig.CapabilityBoundingSet == [
    "CAP_CHOWN"
    "CAP_DAC_OVERRIDE"
    "CAP_FOWNER"
  ];
assert lib.elem
  "d /var/lib/hermes-worker/workspace/.tentaflake-worker/inbox 0770 10000 tfw-gid-10000 -"
  workerCapsule.config.systemd.tmpfiles.rules;
assert lib.elem "tentaflake-workspace-quota-hermes-worker.service"
  workerCapsule.config.systemd.services.docker-hermes-worker.requires;
assert lib.hasInfix "refusing to hide non-empty workspace"
  workerCapsule.config.systemd.services.tentaflake-workspace-quota-prepare-hermes-worker.script;
assert lib.hasInfix " load --input "
  workerCapsule.config.systemd.services.tentaflake-worker-image.serviceConfig.ExecStart;
assert lib.hasInfix "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue" brokerManifest;
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-llm-hermes-brokered".serviceConfig.LoadCredential
  == [
    "agent-token:/run/tentaflake-broker/hermes-brokered/agent-token"
    "provider:/run/agenix/example-provider"
  ];
assert
  brokerCapsule.config.systemd.services."tentaflake-broker-fetch-hermes-brokered".serviceConfig.CapabilityBoundingSet
  == [ ];
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
