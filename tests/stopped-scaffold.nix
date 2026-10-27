# OpenClaw and Pi share lib/stoppedScaffold.nix. The caller's NixOS evaluation
# declares both instances; module-level checks need no further full evaluation.
{
  pkgs,
  builders,
  config,
}:
let
  inherit (pkgs) lib;
  contract = import ../lib/adapterContract.nix { inherit lib; };
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
  scaffold =
    {
      id,
      name,
      version,
      uid,
      command,
      capabilities,
    }:
    let
      adapter = builders.adapters.${id};
      container = "${id}-${name}";
      instance =
        args: backend:
        let
          selected =
            builders.mkAgent
              (
                {
                  adapter = id;
                  inherit name;
                }
                // args
              )
              {
                config.virtualisation.oci-containers.backend = backend;
              };
        in
        {
          record = builtins.head selected.tentaflake.agentDeclarations;
          module = (builtins.head selected.imports) {
            config.virtualisation.oci-containers.backend = backend;
          };
        };
      docker = instance { } "docker";
      podman = instance { } "podman";
      service = docker.module.systemd.services."docker-${container}";
      conflicting = (adapter.build { inherit name; }) {
        config = {
          virtualisation.oci-containers.backend = "docker";
          virtualisation.oci-containers.containers.${container} = { };
          tentaflake.workspaceQuota.agents.${container}.enable = true;
        };
      };
      failed = map (item: item.message) (lib.filter (item: !item.assertion) conflicting.assertions);
    in
    assert (contract id adapter).identity.status == "scaffold";
    assert adapter.identity.version == version;
    assert adapter.artifact.kind == "unselected" && !adapter.artifact.reviewed;
    # Planned upstream facts are metadata only; the scaffold never runs them.
    assert adapter.command == command && adapter.capabilities == capabilities;
    assert !adapter.research.supported && !(adapter.research.configure { }).valid;
    assert rejects (adapter.configuration.generate name { });
    assert adapter.execution.interface == "unverified" && !adapter.execution.automatic;
    assert adapter.evidence.vendorAcceptance == "pending";
    assert docker.record.adapter == id && !docker.record.runnable;
    assert docker.record.container == container;
    assert docker.record.unit == "docker-${container}.service";
    assert docker.record.uid == uid && docker.record.gid == uid;
    assert docker.record.stateDir == "/var/lib/${container}";
    assert docker.record.workspace == "/var/lib/${container}/workspace";
    assert podman.record.unit == "podman-${container}.service";
    assert
      builtins.attrNames docker.module == [
        "assertions"
        "systemd"
      ];
    assert lib.all (item: item.assertion) docker.module.assertions;
    assert builtins.attrNames docker.module.systemd == [ "services" ];
    assert builtins.attrNames podman.module.systemd.services == [ "podman-${container}" ];
    assert service.wantedBy == [ ];
    assert service.serviceConfig.Type == "oneshot" && service.serviceConfig.Restart == "no";
    assert lib.hasInfix "acceptance evidence" service.script && lib.hasInfix "exit 1" service.script;
    assert lib.length failed == 2;
    assert lib.any (lib.hasInfix "cannot share its identity") failed;
    assert lib.any (lib.hasInfix "cannot provision workspace or state quotas") failed;
    assert config.tentaflake.agentInstances.${container}.runnable == false;
    assert !(config.virtualisation.oci-containers.containers ? ${container});
    assert config.systemd.services."docker-${container}".wantedBy == [ ];
    assert lib.all (rule: !(lib.hasInfix "/var/lib/${container}" rule)) config.systemd.tmpfiles.rules;
    assert lib.all (args: rejects (instance args "docker")) [
      { autoStart = true; }
      { settings.debug = true; }
      { stateDir = "/var/lib/${container}/../outside"; }
      {
        image = "example.invalid/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      }
      { extraVolumes = [ "/host:/host:rw" ]; }
      { boardSocket = "/run/other/bridge.sock"; }
      { envFile = "/run/fixture.env"; }
    ];
    true;
  presets = builders.runtimeCatalog.presets;
  # A runtime identity must not share numeric ownership with another runtime;
  # the generic definition default is replaced by each definition's owner.
  reserved = lib.mapAttrsToList (_: preset: preset.ownership.uid) (
    lib.filterAttrs (id: _: id != "generic") presets
  );
in
assert lib.all (
  id:
  presets.${id}.identity.status != "scaffold"
  || !(builders.adapters.${id}.metadata { name = "fixture"; } "docker").runnable
) (builtins.attrNames presets);
assert lib.length (lib.unique reserved) == lib.length reserved;
assert scaffold {
  id = "pi";
  name = "pilot";
  version = "1.4.0";
  uid = 10100;
  command = [ ];
  capabilities = [ ];
};
assert scaffold {
  id = "openclaw";
  name = "assistant";
  version = "2026.9.7";
  uid = 1000;
  command = [
    "node"
    "openclaw.mjs"
    "gateway"
  ];
  capabilities = [
    "files"
    "shell"
    "terminal"
    "worker"
  ];
};
true
