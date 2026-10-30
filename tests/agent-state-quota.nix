{
  pkgs,
  eval,
}:
let
  inherit (pkgs) lib;
  mkAgent = import ../lib/mkAgent.nix { inherit pkgs lib; };
  fixture =
    extra:
    eval (
      [
        (mkAgent {
          adapter = "generic";
          name = "generic-quota";
          definition = {
            schemaVersion = 1;
            image = "registry.example.invalid/fixture@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
            command = [ "fixture" ];
          };
        })
        (mkAgent {
          adapter = "hermes";
          name = "hermes-quota";
          autoStart = false;
          seedDir = ./fixtures/adapter-seed;
        })
        (mkAgent {
          adapter = "zeroclaw";
          name = "zero-quota";
          autoStart = false;
          seedDir = ./fixtures/adapter-seed;
        })
        {
          tentaflake = {
            workspaceQuota.agents = {
              agent-generic-quota = {
                enable = true;
                workspace = "/var/lib/tentaflake/agents/generic-quota/workspace";
                sizeMiB = 128;
                state = {
                  path = "/var/lib/tentaflake/agents/generic-quota/state";
                  sizeMiB = 128;
                };
              };
              agent-hermes-quota = {
                enable = true;
                workspace = "/var/lib/tentaflake/agents/hermes-quota/workspace";
                sizeMiB = 128;
                state = {
                  path = "/var/lib/tentaflake/agents/hermes-quota";
                  sizeMiB = 128;
                };
              };
              agent-zero-quota = {
                enable = true;
                workspace = "/var/lib/tentaflake/agents/zero-quota/data";
                sizeMiB = 128;
                ownerUid = 65534;
                ownerGid = 65534;
                state = {
                  path = "/var/lib/tentaflake/agents/zero-quota";
                  sizeMiB = 128;
                };
              };
            };
            worker.agents.agent-hermes-quota = {
              enable = true;
              workspace = "/var/lib/tentaflake/agents/hermes-quota/workspace";
            };
            backup = {
              enable = true;
              paths = [ "/var/lib/tentaflake/agents/generic-quota" ];
              repositoryFile = "/run/fixture-repository";
              passwordFile = "/run/fixture-password";
            };
          };
        }
      ]
      ++ extra
    );
  config = fixture [ ];
  services = config.systemd.services;
  mount = path: lib.findFirst (entry: entry.where == path) null config.systemd.mounts;
  failed = config: lib.filter (item: !item.assertion) config.assertions;
  refuses =
    text: change: lib.any (item: lib.hasInfix text item.message) (failed (fixture [ change ]));
  legacy = fixture [
    { tentaflake.workspaceQuota.agents.agent-hermes-quota.state = lib.mkForce null; }
  ];
  small = fixture [
    { tentaflake.workspaceQuota.agents.agent-generic-quota.state.sizeMiB = lib.mkForce 32; }
  ];
  backupPaths = [
    "/var/lib/tentaflake/agents/generic-quota"
    "/var/lib/tentaflake/agents/generic-quota/workspace"
    "/var/lib/tentaflake/agents/generic-quota/state"
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
  config.tentaflake.agentInstances.agent-generic-quota.stateStorage
  == "/var/lib/tentaflake/agents/generic-quota/state";
assert
  config.tentaflake.agentInstances.agent-hermes-quota.stateStorage
  == "/var/lib/tentaflake/agents/hermes-quota";
assert
  config.tentaflake.agentInstances.agent-zero-quota.stateDirectories == [
    ".zeroclaw"
    ".zeroclaw/data"
    "data"
  ];
assert
  (mount "/var/lib/tentaflake/agents/generic-quota/state").what
  == "/var/lib/tentaflake-workspace-volumes/state/agent-generic-quota.img";
assert
  (mount "/var/lib/tentaflake/agents/generic-quota/state").options == "loop,nodev,nosuid,noatime";
assert
  (mount "/var/lib/tentaflake/agents/generic-quota/workspace").what
  == "/var/lib/tentaflake-workspace-volumes/agent-generic-quota.img";
assert lib.elem "tentaflake-state-quota-agent-generic-quota.service"
  services.tentaflake-workspace-quota-agent-generic-quota.requires;
assert
  !(lib.elem "tentaflake-state-quota-agent-generic-quota.service" services.tentaflake-workspace-quota-prepare-agent-generic-quota.requires);
assert lib.elem "tentaflake-state-quota-agent-hermes-quota.service"
  services.tentaflake-workspace-quota-prepare-agent-hermes-quota.requires;
assert lib.elem "tentaflake-workspace-quota-agent-hermes-quota.service"
  services.agent-hermes-quota-heal-uid.requires;
assert lib.elem "tentaflake-workspace-quota-agent-hermes-quota.service"
  services.agent-hermes-quota-seed.requires;
assert lib.elem "tentaflake-workspace-quota-agent-zero-quota.service"
  services.agent-zero-quota-seed.requires;
assert
  !(lib.any (
    rule: lib.hasInfix "/var/lib/tentaflake/agents/hermes-quota/workspace " rule
  ) config.systemd.tmpfiles.rules);
assert
  !(lib.any (
    rule: lib.hasInfix "/var/lib/tentaflake/agents/hermes-quota/workspace/.tentaflake-worker" rule
  ) config.systemd.tmpfiles.rules);
assert
  !(lib.any (
    rule: lib.hasInfix "/var/lib/tentaflake/agents/zero-quota/.zeroclaw " rule
  ) config.systemd.tmpfiles.rules);
assert config.services.restic.backups.tentaflake.paths == backupPaths;
assert
  config.systemd.services.restic-backups-tentaflake.unitConfig.RequiresMountsFor == backupPaths;
assert
  config.systemd.services.restic-backups-tentaflake.unitConfig.AssertPathIsMountPoint
  == lib.tail backupPaths;
assert failed legacy == [ ];
assert !(legacy.systemd.services ? tentaflake-state-quota-agent-hermes-quota);
assert
  !(lib.elem "tentaflake-state-quota-agent-hermes-quota.service" legacy.systemd.services.tentaflake-workspace-quota-agent-hermes-quota.requires);
assert lib.elem "d /var/lib/tentaflake/agents/hermes-quota/workspace 0700 10000 10000 -"
  legacy.systemd.tmpfiles.rules;
assert
  !(builtins.tryEval small.tentaflake.workspaceQuota.agents.agent-generic-quota.state.sizeMiB)
  .success;
assert refuses "declared adapter state source" {
  tentaflake.workspaceQuota.agents.agent-generic-quota.state.path = lib.mkForce "/var/lib/other";
};
assert refuses "declared adapter state source" {
  tentaflake.workspaceQuota.agents.agent-generic-quota.ownerUid = lib.mkForce 12000;
};
assert refuses "normalized private state" {
  tentaflake.workspaceQuota.agents.agent-generic-quota.state.path =
    lib.mkForce "/var/lib/tentaflake/agents/generic-quota/workspace/state";
};
assert
  !(builtins.tryEval (
    builtins.deepSeq (failed (fixture [
      {
        tentaflake.workspaceQuota.agents.agent-generic-quota.state.path =
          lib.mkForce "/var/lib/tentaflake/agents/generic-quota/../state";
      }
    ])) true
  )).success;
assert refuses "may not overlap" {
  tentaflake.workspaceQuota.agents.agent-generic-quota.state.path =
    lib.mkForce "/var/lib/tentaflake/agents/hermes-quota/other";
};
assert refuses "intended for balanced/strict" { tentaflake.security.profile = lib.mkForce "dev"; };
{
  prepareScript = services.tentaflake-state-quota-prepare-agent-generic-quota.script;
  ownerScript = services.tentaflake-state-quota-agent-hermes-quota.script;
}
