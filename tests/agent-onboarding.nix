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
  result = plan generic;
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
in
assert result.valid && result.errors == [ ];
assert (lib.head result.instances).container == "generic-from-json";
assert (lib.head result.instances).requiresHostUpdate;
assert (plan hermes).valid;
assert !(lib.head (plan scaffold).instances).runnable;
assert rejects (plan (generic // { schemaVersion = 2; }));
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
true
