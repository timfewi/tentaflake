{ pkgs }:
let
  inherit (pkgs) lib;
  plan = import ../lib/agentPlan.nix { inherit pkgs; };
  generic = builtins.fromJSON (builtins.readFile ./fixtures/adapter-input/generic.json);
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
  piScaffold = scaffold // {
    agents = [
      {
        adapter = "pi";
        name = "research";
        autoStart = false;
      }
    ];
  };
  result = plan generic;
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
in
assert result.valid && result.errors == [ ];
assert (lib.head result.instances).container == "agent-from-json";
assert (lib.head result.instances).requiresHostUpdate;
assert (plan hermes).valid;
assert !(lib.head (plan scaffold).instances).runnable;
assert !(lib.head (plan piScaffold).instances).runnable;
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
true
