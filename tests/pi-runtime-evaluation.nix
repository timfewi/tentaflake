{ pkgs }:
let
  inherit (pkgs) lib;
  transport = import ../lib/piTransport.nix { inherit pkgs lib; };
  security = import ../lib/containerSecurity.nix { inherit lib; };
  docker = {
    tentaflake.piRuntime = {
      enable = true;
      transport = transport.version;
    };
    virtualisation = {
      oci-containers.backend = "docker";
      docker.daemon.settings.runtimes.runsc.path = lib.getExe' pkgs.gvisor "runsc";
      docker.daemon.settings.runtimes.runsc-pi.path = transport.path;
    };
  };
  podman = lib.recursiveUpdate docker {
    virtualisation = {
      oci-containers.backend = "podman";
      containers.containersConf.settings.engine.runtimes.runsc = [
        (lib.getExe' pkgs.gvisor "runsc")
      ];
      containers.containersConf.settings.engine.runtimes.runsc-pi = [ transport.path ];
    };
  };
  rejects = base: change: !(transport.supported (lib.recursiveUpdate base change));
  image = "tentaflake-pi:${lib.concatStrings (lib.replicate 64 "a")}";
  bundle = pkgs.runCommand "pi-evaluation-only-image-proof" {
    passthru = {
      imageVerificationVersion = 1;
      imageSelector = image;
      runtimePackage = pkgs.hello;
    };
  } "mkdir $out";
  apply =
    backend: runtimeName:
    security.apply {
      inherit backend runtimeName;
      profile = "balanced";
      name = "agent-fixture";
      owner = "10100:10100";
      baseConfig = {
        inherit image;
        imageFile = "${bundle}/image.tar.gz";
      };
      reviewedImage = { inherit bundle image; };
      allowedWritableSources = [ ];
      allowedWritableDestinations = [ ];
      pidsLimit = 128;
      resources = {
        memory = "256m";
        memorySwap = "256m";
        cpus = "1";
        tmpfsSize = "16m";
        runTmpfsSize = "16m";
        nofile = 1024;
      };
    };
  selected = apply "docker" transport.name;
  selectedPodman = apply "podman" transport.name;
  ordinary = apply "docker" "runsc";
  valid = result: lib.all (entry: entry.assertion) result.assertions;
  evaluate =
    extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      specialArgs.researchFlake = null;
      modules = [
        ../modules/default.nix
        {
          system.stateVersion = "26.05";
          virtualisation.oci-containers.backend = "docker";
          tentaflake = {
            adminUser = "operator";
            boot.enable = false;
            locale.enable = false;
            networking.enable = false;
            nixSettings.enable = false;
            packages.enable = false;
            shell.enable = false;
          };
        }
        extra
      ];
    }).config;
  defaultHost = evaluate { };
  piHost = evaluate {
    tentaflake.piRuntime = {
      enable = true;
      transport = transport.version;
    };
  };
  wrongHost = evaluate {
    tentaflake.piRuntime = {
      enable = true;
      transport = transport.version;
    };
    virtualisation.docker.daemon.settings.runtimes.runsc-pi.path = lib.mkForce "/bin/runsc";
  };
  podmanHost = evaluate {
    tentaflake.piRuntime = {
      enable = true;
      transport = transport.version;
    };
    virtualisation.oci-containers.backend = lib.mkForce "podman";
  };
  # Synthetic reviewed image admission exercises the actual security manifest,
  # not vendor/runtime acceptance or a duplicate of the mount predicate.
  piMounts = [
    "/run/tentaflake-pi/agent-fixture/control:/run/tentaflake-pi/control:rw"
    "/run/tentaflake-pi/agent-fixture/api-token:/run/tentaflake-pi/api-token:ro"
    "/run/tentaflake-pi/agent-fixture/broker-token:/run/tentaflake-pi/broker-token:ro"
  ];
  manifestHost =
    runnable: volumes:
    evaluate {
      tentaflake = {
        piRuntime = {
          enable = true;
          transport = transport.version;
          package = pkgs.hello;
          inherit image bundle;
        };
        agentDeclarations = [
          {
            inherit runnable;
            adapter = "pi";
            name = "fixture";
            container = "agent-fixture";
            unit = "docker-agent-fixture.service";
            stateDir = "/var/lib/tentaflake/agents/fixture";
            stateStorage = "/var/lib/tentaflake/agents/fixture/state";
            workspace = "/var/lib/tentaflake/agents/fixture/workspace";
            uid = 10100;
            gid = 10100;
          }
        ];
      };
      virtualisation.oci-containers.containers.agent-fixture = selected.container // {
        inherit volumes;
      };
      systemd.services.docker-agent-fixture.serviceConfig.ExecStartPre = [
        (import ../lib/verifyLocalImage.nix {
          inherit
            lib
            pkgs
            bundle
            image
            ;
          backend = "docker";
        })
      ];
    };
  manifestAgent =
    configuration:
    lib.splitString "\t" (
      lib.findFirst (line: lib.hasPrefix "agent\tagent-fixture\t" line)
        (throw "Pi security manifest is missing agent-fixture")
        (lib.splitString "\n" configuration.environment.etc."tentaflake/security.tsv".text)
    );
  mountsSafe = configuration: lib.elemAt (manifestAgent configuration) 10 == "true";
  admittedManifest = manifestHost true piMounts;
  stoppedManifest = manifestHost false piMounts;
  changedMounts = from: to: map (lib.replaceStrings [ from ] [ to ]) piMounts;
  otherAgentMounts = changedMounts "/agent-fixture/" "/agent-other/";
  destinationDriftMounts = map (
    mount: lib.replaceStrings [ ":/run/tentaflake-pi/" ] [ ":/run/tentaflake-pi/drift/" ] mount
  ) piMounts;
  writableTokenMounts = lib.tail piMounts;
in
assert !(transport.supported defaultHost);
assert !(defaultHost.virtualisation.docker.daemon.settings.runtimes ? runsc-pi);
assert
  defaultHost.virtualisation.docker.daemon.settings.runtimes.runsc.path
  == lib.getExe' pkgs.gvisor "runsc";
assert
  piHost.virtualisation.docker.daemon.settings.runtimes.runsc
  == defaultHost.virtualisation.docker.daemon.settings.runtimes.runsc;
assert transport.supported piHost;
assert transport.supported podmanHost;
assert !(transport.supported wrongHost);
assert transport.supported docker;
assert transport.supported podman;
assert !(transport.supported { });
assert transport.containerSupported { podman.user = null; };
assert transport.containerSupported { podman.user = "root"; };
assert !(transport.containerSupported { podman.user = "operator"; });
assert !(transport.containerSupported { extraOptions = [ "--userns=host" ]; });
assert !(transport.containerSupported { preRunExtraOptions = [ "--uidmap=0:100000:65536" ]; });
assert
  !(transport.containerSupported {
    extraOptions = [
      "--gidmap"
      "0:100000:65536"
    ];
  });
assert rejects docker { tentaflake.piRuntime.transport = null; };
assert rejects docker {
  virtualisation.docker.daemon.settings.runtimes.runsc-pi.path = "/bin/runsc";
};
assert rejects docker {
  virtualisation.docker.daemon.settings.runtimes.runsc-pi.runtimeArgs = [ "--host-uds=all" ];
};
assert rejects docker { virtualisation.docker.daemon.settings."userns-remap" = "default"; };
assert rejects docker { virtualisation.docker.rootless.enable = true; };
assert rejects docker { virtualisation.docker.extraOptions = "--userns-remap=default"; };
assert rejects docker { systemd.services.docker.serviceConfig.User = "operator"; };
assert rejects docker { virtualisation.docker.daemon.settings."default-runtime" = "runsc-pi"; };
assert rejects docker {
  virtualisation.docker.daemon.settings.runtimes.runsc.path = transport.path;
};
assert rejects docker {
  virtualisation.docker.daemon.settings.runtimes.alias.path = transport.path;
};
assert rejects docker {
  virtualisation.docker.daemon.settings.runtimes.alias = {
    path = lib.getExe' pkgs.gvisor "runsc";
    runtimeArgs = [ "--host-uds=all" ];
  };
};
assert rejects docker {
  virtualisation.docker.daemon.settings = {
    runtimes.alias.path = transport.path;
    "default-runtime" = "alias";
  };
};
assert rejects podman {
  virtualisation.containers.containersConf.settings.engine.runtimes.runsc-pi = [
    "/bin/runsc"
    "--host-uds=all"
  ];
};
assert rejects podman {
  virtualisation.containers.containersConf.settings.containers.userns = "auto";
};
assert rejects podman {
  virtualisation.containers.containersConf.settings.engine.runtime = "runsc-pi";
};
assert rejects podman {
  virtualisation.containers.containersConf.settings.engine.runtimes.runsc = [ transport.path ];
};
assert rejects podman {
  virtualisation.containers.containersConf.settings.engine = {
    runtimes.alias = [ transport.path ];
    runtime = "alias";
  };
};
assert valid selected && valid selectedPodman;
assert !(valid (apply "docker" "arbitrary"));
assert lib.elem "--runtime=runsc-pi" selected.container.extraOptions;
assert
  selectedPodman.container.preRunExtraOptions == [
    "--runtime"
    "runsc-pi"
  ];
assert lib.elem "--runtime=runsc" ordinary.container.extraOptions;
assert selected.container.pull == "never";
assert lib.all (option: lib.elem option selected.container.extraOptions) [
  "--read-only"
  "--network=none"
  "--security-opt=no-new-privileges:true"
  "--memory=256m"
  "--memory-swap=256m"
  "--cpus=1"
  "--pids-limit=128"
];
assert lib.assertMsg (
  lib.elemAt (manifestAgent admittedManifest) 11 == "true"
) "Pi manifest fixture must satisfy the full runtime admission predicate";
assert lib.assertMsg (mountsSafe admittedManifest)
  "admitted Pi control and read-only token mounts must project mounts_safe=true";
assert lib.assertMsg (
  lib.elemAt (manifestAgent stoppedManifest) 11 == "false" && !mountsSafe stoppedManifest
) "stopped Pi declarations must not receive runtime mount grants";
assert lib.assertMsg (lib.all (
  mount: !mountsSafe (manifestHost true [ mount ])
) otherAgentMounts) "Pi manifest must reject every other-agent control or token source";
assert lib.assertMsg (lib.all (
  mount: !mountsSafe (manifestHost true [ mount ])
) destinationDriftMounts) "Pi manifest must reject destination drift for control and both tokens";
assert lib.assertMsg (lib.all (
  mount: !mountsSafe (manifestHost true [ (lib.replaceStrings [ ":ro" ] [ ":rw" ] mount) ])
) writableTokenMounts) "Pi manifest must reject writable API and broker token mounts";
assert lib.assertMsg (
  !mountsSafe (
    manifestHost true [
      "/run/tentaflake-pi/agent-fixture/control:/run/tentaflake-pi/control:ro"
    ]
  )
) "Pi manifest control mount grant must retain its exact writable mode";
pkgs.runCommand "pi-runtime-evaluation" { } ''
  mkdir "$out"
  echo 'Configured transport only; disposable VM acceptance is separate.' > "$out/scope"
''
