{
  pkgs,
  eval,
  builders,
}:
let
  inherit (pkgs) lib;
  fixture =
    extra:
    eval (
      [
        (builders.mkAgent {
          adapter = "generic";
          name = "quota";
          definition = {
            schemaVersion = 1;
            image = builders.constants.hermesImage;
            command = [ "fixture" ];
          };
        })
        (builders.mkHermesAgent {
          name = "quota";
          autoStart = false;
          seedDir = ./fixtures/adapter-seed;
        })
        (builders.mkZeroClawAgent {
          name = "quota";
          autoStart = false;
          seedDir = ./fixtures/adapter-seed;
        })
        {
          tentaflake = {
            workspaceQuota.agents = {
              generic-quota = {
                enable = true;
                workspace = "/var/lib/generic-quota/workspace";
                sizeMiB = 128;
                state = {
                  path = "/var/lib/generic-quota/state";
                  sizeMiB = 128;
                };
              };
              hermes-quota = {
                enable = true;
                workspace = "/var/lib/hermes-quota/workspace";
                sizeMiB = 128;
                state = {
                  path = "/var/lib/hermes-quota";
                  sizeMiB = 128;
                };
              };
              zeroclaw-quota = {
                enable = true;
                workspace = "/var/lib/zeroclaw-quota/data";
                sizeMiB = 128;
                ownerUid = 65534;
                ownerGid = 65534;
                state = {
                  path = "/var/lib/zeroclaw-quota";
                  sizeMiB = 128;
                };
              };
            };
            worker.agents.hermes-quota = {
              enable = true;
              workspace = "/var/lib/hermes-quota/workspace";
            };
            backup = {
              enable = true;
              paths = [ "/var/lib/generic-quota" ];
              repositoryFile = "/run/fixture-repository";
              passwordFile = "/run/fixture-password";
            };
          };
        }
      ]
      ++ extra
    );
  config = fixture [ ];
  emptyReserve = lib.genAttrs [
    "memoryBytes"
    "memorySwapBytes"
    "nanoCpus"
    "pids"
    "workspaceBytes"
    "stateBytes"
  ] (_: 0);
  admitted = fixture [
    {
      tentaflake.security.admission = {
        authorityEpoch = "fixture-authority";
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
    }
  ];
  # Inspect JSON values only; the generated writeText derivation retains every
  # tool/helper dependency. fromJSON rejects contextual strings even in tests.
  plan = builtins.fromJSON (
    builtins.unsafeDiscardStringContext admitted.tentaflake.admissionManifest.text
  );
  services = config.systemd.services;
  stateHelpers = config.tentaflake.admissionVolumes."state:generic-quota".helpers;
  prepareLines = lib.splitString "\n" services.tentaflake-state-quota-prepare-generic-quota.script;
  ownerHelper = config.tentaflake.admissionVolumes."state:hermes-quota".helpers.owner;
  mount = path: lib.findFirst (entry: entry.where == path) null config.systemd.mounts;
  failed = config: lib.filter (item: !item.assertion) config.assertions;
  refuses =
    text: change: lib.any (item: lib.hasInfix text item.message) (failed (fixture [ change ]));
  legacy = fixture [ { tentaflake.workspaceQuota.agents.hermes-quota.state = lib.mkForce null; } ];
  small = fixture [
    { tentaflake.workspaceQuota.agents.generic-quota.state.sizeMiB = lib.mkForce 32; }
  ];
  backupPaths = [
    "/var/lib/generic-quota"
    "/var/lib/generic-quota/workspace"
    "/var/lib/generic-quota/state"
  ];
in
assert failed config == [ ];
assert
  builtins.length (
    lib.filter (
      entry: lib.hasPrefix "/var/lib/tentaflake-workspace-volumes/" entry.what
    ) config.systemd.mounts
  ) == 6;
assert
  config.tentaflake.agentInstances.generic-quota.stateStorage == "/var/lib/generic-quota/state";
assert config.tentaflake.agentInstances.hermes-quota.stateStorage == "/var/lib/hermes-quota";
assert
  config.tentaflake.agentInstances.zeroclaw-quota.stateDirectories == [
    ".zeroclaw"
    ".zeroclaw/data"
    "data"
  ];
assert
  (mount "/var/lib/generic-quota/state").what
  == "/var/lib/tentaflake-workspace-volumes/state/generic-quota.img";
assert (mount "/var/lib/generic-quota/state").options == "loop,nodev,nosuid,noatime";
assert
  (mount "/var/lib/generic-quota/workspace").what
  == "/var/lib/tentaflake-workspace-volumes/generic-quota.img";
assert lib.elem "tentaflake-state-quota-generic-quota.service"
  services.tentaflake-workspace-quota-generic-quota.requires;
assert
  !(lib.elem "tentaflake-state-quota-generic-quota.service" services.tentaflake-workspace-quota-prepare-generic-quota.requires);
assert lib.elem "tentaflake-state-quota-hermes-quota.service"
  services.tentaflake-workspace-quota-prepare-hermes-quota.requires;
assert lib.elem "tentaflake-workspace-quota-hermes-quota.service"
  services.hermes-quota-heal-uid.requires;
assert lib.elem "tentaflake-workspace-quota-hermes-quota.service"
  services.seed-hermes-quota.requires;
assert lib.elem "tentaflake-workspace-quota-zeroclaw-quota.service"
  services.seed-zeroclaw-quota.requires;
assert
  !(lib.any (
    rule: lib.hasInfix "/var/lib/hermes-quota/workspace " rule
  ) config.systemd.tmpfiles.rules);
assert
  !(lib.any (
    rule: lib.hasInfix "/var/lib/hermes-quota/workspace/.tentaflake-worker" rule
  ) config.systemd.tmpfiles.rules);
assert
  !(lib.any (
    rule: lib.hasInfix "/var/lib/zeroclaw-quota/.zeroclaw " rule
  ) config.systemd.tmpfiles.rules);
assert config.services.restic.backups.tentaflake.paths == backupPaths;
assert
  config.systemd.services.restic-backups-tentaflake.unitConfig.RequiresMountsFor == backupPaths;
assert
  config.systemd.services.restic-backups-tentaflake.unitConfig.AssertPathIsMountPoint
  == lib.tail backupPaths;
assert failed legacy == [ ];
assert !(legacy.systemd.services ? tentaflake-state-quota-hermes-quota);
assert
  !(lib.elem "tentaflake-state-quota-hermes-quota.service" legacy.systemd.services.tentaflake-workspace-quota-hermes-quota.requires);
assert lib.elem "d /var/lib/hermes-quota/workspace 0700 10000 10000 -"
  legacy.systemd.tmpfiles.rules;
assert
  !(builtins.tryEval small.tentaflake.workspaceQuota.agents.generic-quota.state.sizeMiB).success;
assert refuses "declared adapter state source" {
  tentaflake.workspaceQuota.agents.generic-quota.state.path = lib.mkForce "/var/lib/other";
};
assert refuses "declared adapter state source" {
  tentaflake.workspaceQuota.agents.generic-quota.ownerUid = lib.mkForce 12000;
};
assert refuses "normalized private state" {
  tentaflake.workspaceQuota.agents.generic-quota.state.path =
    lib.mkForce "/var/lib/generic-quota/workspace/state";
};
assert
  !(builtins.tryEval (
    builtins.deepSeq (failed (fixture [
      {
        tentaflake.workspaceQuota.agents.generic-quota.state.path =
          lib.mkForce "/var/lib/generic-quota/../state";
      }
    ])) true
  )).success;
assert refuses "may not overlap" {
  tentaflake.workspaceQuota.agents.generic-quota.state.path =
    lib.mkForce "/var/lib/hermes-quota/other";
};
assert refuses "intended for balanced/strict" { tentaflake.security.profile = lib.mkForce "dev"; };
assert lib.elem "  exec ${stateHelpers.create}" prepareLines;
assert lib.elem "  exec ${stateHelpers.verify}" prepareLines;
assert services.tentaflake-state-quota-hermes-quota.script == "exec ${ownerHelper}";
assert config.tentaflake.admissionManifest == null;
assert plan.schemaVersion == 1;
assert plan.policy.generation == "fixture-authority";
assert builtins.stringLength plan.policy.revision == 71;
assert plan.policy.maxActive == 2;
assert plan.policy.grants == [ ];
assert builtins.length plan.volumes == 6;
assert plan.instances.generic-quota.ownerId == "host-root";
assert plan.instances.generic-quota.resources.workspaceBytes == 134217728;
assert plan.instances.generic-quota.resources.stateBytes == 134217728;
assert
  map (requirement: requirement.id) plan.instances.generic-quota.storage == [
    "state:generic-quota"
    "workspace:generic-quota"
  ];
assert lib.all (
  volume:
  volume.spec.ownerId == "host-root" && builtins.stringLength volume.spec.bindingRevision == 71
) plan.volumes;
{
  quotaHelpers = {
    inherit (stateHelpers) create verify;
    owner = ownerHelper;
  };
}
