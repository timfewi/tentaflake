# Stopped creation and recovery only: no OCI image acquisition or agent start.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  builders = self.lib.x86_64-linux;
  source = "generic-creation-source";
  reserve = lib.genAttrs [
    "memoryBytes"
    "memorySwapBytes"
    "nanoCpus"
    "pids"
    "workspaceBytes"
    "stateBytes"
  ] (_: 0);
in
{
  imports = [
    self.nixosModules.default
    (builders.mkAgent {
      adapter = "generic";
      name = "creation-source";
      definition = {
        schemaVersion = 1;
        image = builders.baseImages.image {
          id = "debian-13";
          system = "x86_64-linux";
        };
        command = [ "/bin/true" ];
        lifecycle = "stopped";
        capabilities = [ ];
        ownership = {
          uid = 65534;
          gid = 65534;
        };
        resources = {
          memory = "256m";
          memorySwap = "256m";
          cpus = "1";
          nofile = 128;
          pidsLimit = 128;
          tmpfsSize = "8m";
          runTmpfsSize = "8m";
        };
      };
    })
  ];

  # Keep real Nix registration for prebuilt closures. Empty build-users-group
  # avoids chown on the shared store; internal remounts stay service-private.
  nix = {
    enable = true;
    nrBuildUsers = 0;
    settings = {
      build-users-group = lib.mkForce "";
      max-jobs = lib.mkForce 0;
      builders = lib.mkForce "";
      substituters = lib.mkForce [ ];
      substitute = false;
    };
  };
  virtualisation = {
    writableStore = false;
    fileSystems."/nix/store".options = [ "ro" ];
    oci-containers.backend = "docker";
    docker = {
      enable = true;
      daemon.settings = {
        "live-restore" = false;
        features."containerd-snapshotter" = false;
      };
    };
    memorySize = 1024;
    diskSize = 4096;
  };
  systemd = {
    services = {
      register-nix-paths = {
        preStart = ''
          test -d /nix/store/.links && test ! -L /nix/store/.links
        '';
        serviceConfig.PrivateMounts = true;
      };
      nix-daemon = {
        preStart = ''
          test -d /nix/store/.links && test ! -L /nix/store/.links
        '';
        serviceConfig.PrivateMounts = true;
      };
      # These are real guarded owners; only their automatic activation is off.
      "tentaflake-state-quota-${source}".wantedBy = lib.mkForce [ ];
      "tentaflake-workspace-quota-${source}".wantedBy = lib.mkForce [ ];
    };
    # Both store mounts must remain available to the final shutdown executable.
    # Persistent volumes retain their ordinary guarded stop/unmount behavior.
    units =
      lib.genAttrs
        [
          "nix-store.mount"
          "nix-.ro\\x2dstore.mount"
        ]
        (_: {
          overrideStrategy = "asDropin";
          text = ''
            [Unit]
            DefaultDependencies=no
          '';
        });
  };
  environment.systemPackages = [
    (import ./creation-harness.nix { inherit pkgs; })
    pkgs.btrfs-progs
    pkgs.python3
    pkgs.util-linux
  ];

  tentaflake = {
    hostName = "creation-host";
    adminUser = "admin";
    boot.enable = false;
    modernConsole.enable = false;
    nixSettings.enable = false;
    shell.enable = true;
    security.admission = {
      authorityEpoch = "creation-vm";
      physicalSpaceHeadroomBytes = 67108864;
      capacity = {
        memoryBytes = 2147483648;
        memorySwapBytes = 2147483648;
        nanoCpus = 8000000000;
        pids = 1024;
        workspaceBytes = 2147483648;
        stateBytes = 2147483648;
      };
      hostReserve = reserve;
      researchReserve = reserve;
      maxActive = 8;
      creationTemplates.fixture = {
        sourceInstance = source;
        imageId = "debian-13";
        principals = [ "creation-operator" ];
        issuedActions = [
          "environments.read"
          "workspaces.read"
          "workspaces.retain"
        ];
      };
    };
    workspaceQuota.agents.${source} = {
      enable = true;
      workspace = "/var/lib/${source}/workspace";
      sizeMiB = 128;
      ownerUid = 65534;
      ownerGid = 65534;
      state = {
        path = "/var/lib/${source}/state";
        sizeMiB = 128;
      };
    };
  };
  specialisation.without-template.configuration.tentaflake.security.admission.creationTemplates =
    lib.mkForce
      { };
  assertions = [
    {
      assertion =
        !config.tentaflake.api.enable
        && !config.virtualisation.oci-containers.containers.${source}.autoStart;
      message = "The creation fixture must not enable the HTTP service or start its source container.";
    }
  ];
  system.stateVersion = "26.05";
}
