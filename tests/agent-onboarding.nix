{ pkgs }:
let
  inherit (pkgs) lib;
  plan = import ../lib/agentPlan.nix { inherit pkgs; };
  generic = builtins.fromTOML (builtins.readFile ./fixtures/system-generic/tentaflake.toml);
  hermes = {
    schemaVersion = 1;
    agents = [
      {
        adapter = "hermes";
        name = "preset";
        autoStart = false;
      }
    ];
  };
  scaffold = {
    schemaVersion = 1;
    agents = [
      {
        adapter = "openclaw";
        name = "preset";
        autoStart = false;
      }
    ];
  };
  piDurableScaffold = scaffold // {
    agents = [
      {
        adapter = "pi-durable";
        name = "research";
        autoStart = false;
      }
    ];
  };
  result = plan generic;
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
  withState =
    stateDir:
    hermes
    // {
      agents = [ ((lib.head hermes.agents) // { inherit stateDir; }) ];
    };
  existing = {
    container = "agent-existing";
    stateDir = "/var/lib/tentaflake/agents/existing";
    stateStorage = "/var/lib/tentaflake/agents/storage";
    workspace = "/var/lib/tentaflake/agents/workspace";
  };
  againstInstalled = import ../lib/agentPlan.nix {
    inherit pkgs;
    existingInstances = [ existing ];
  };
  desired = {
    adapter = "hermes";
    name = "desired";
    autoStart = true;
    stateDir = "/var/lib/tentaflake/agents/desired/child";
  };
  againstDesired = import ../lib/agentPlan.nix {
    inherit pkgs;
    desiredAgents = [ desired ];
  };
  isolationRegressions = {
    distinctNamesSharingState = !(againstInstalled (withState existing.stateDir)).valid;
    incomingStateNestedInExisting = !(againstInstalled (withState "${existing.stateDir}/child")).valid;
    incomingStateContainsExisting =
      !(againstDesired (withState "/var/lib/tentaflake/agents/desired")).valid;
    incomingStateAliasesExistingStorage = !(againstInstalled (withState existing.stateStorage)).valid;
    incomingStateNestedInExistingStorage =
      !(againstInstalled (withState "${existing.stateStorage}/child")).valid;
    incomingStateAliasesExistingWorkspace = !(againstInstalled (withState existing.workspace)).valid;
    desiredStateReservedWithoutActiveInventory = !(againstDesired (withState desired.stateDir)).valid;
    desiredRunningAgentDoesNotNeedToBeStopped =
      (againstDesired (withState "/var/lib/tentaflake/agents/separate")).valid;
    adjacentStatePathsRemainSeparate =
      (againstInstalled (withState "${existing.stateDir}-sibling")).valid;
  };
in
assert result.valid && result.errors == [ ];
assert (lib.head result.instances).container == "agent-from-json";
assert (lib.head result.instances).requiresHostUpdate;
assert (plan hermes).valid;
assert !(lib.head (plan scaffold).instances).runnable;
assert !(lib.head (plan piDurableScaffold).instances).runnable;
assert lib.all (value: value) (lib.attrValues isolationRegressions);
assert rejects (plan (generic // { schemaVersion = 2; }));
assert lib.all (schemaVersion: rejects (plan (generic // { inherit schemaVersion; }))) [
  1.0
  "1"
  true
];
assert rejects (
  plan (
    hermes
    // {
      agents = [
        {
          adapter = "hermes";
          name = "unsafe";
          autoStart = false;
          stateDir = "/var/lib/docker";
        }
      ];
    }
  )
);
assert rejects (
  plan (
    hermes
    // {
      agents = [
        {
          adapter = "hermes";
          name = "unsafe";
          autoStart = false;
          extraVolumes = [ "/home:/host:rw" ];
        }
      ];
    }
  )
);
assert
  !(plan (
    hermes
    // {
      agents = [
        {
          adapter = "hermes";
          name = "running";
          autoStart = true;
        }
      ];
    }
  )).valid;
assert rejects (plan (generic // { agents = generic.agents ++ generic.agents; }));
assert rejects (
  plan (
    hermes
    // {
      agents = hermes.agents ++ scaffold.agents;
    }
  )
);
assert rejects (plan {
  agents = [ ];
});
assert rejects (plan {
  schemaVersion = 1;
});
assert rejects (plan (hermes // { hermes = [ ]; }));
assert rejects (plan (hermes // { zeroclaw = [ ]; }));
assert (plan (hermes // { _securityNote = "Public fixture metadata only."; })).valid;
assert rejects (plan (hermes // { _securityNote = true; }));
assert rejects (plan (hermes // { _unknownMetadata = "not admitted"; }));
true
