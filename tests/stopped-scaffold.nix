# OpenClaw and Pi share lib/stoppedScaffold.nix. The caller's NixOS evaluation
# declares both instances; module-level checks need no further full evaluation.
{
  pkgs,
  builders,
  config,
}:
let
  inherit (pkgs) lib;
  mkAgent = import ../lib/mkAgent.nix { inherit pkgs lib; };
  adapters = import ../adapters { inherit pkgs lib; };
  contract = import ../lib/adapterContract.nix { inherit lib; };
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
  # Adapters may return literal NixOS modules with conditional config. Resolve
  # those definitions instead of relying on their pre-module attribute shape.
  moduleConfig =
    module:
    (lib.evalModules {
      modules = [
        {
          config._module.freeformType = lib.types.attrsOf lib.types.anything;
          options = {
            assertions = lib.mkOption {
              type = lib.types.listOf lib.types.anything;
              default = [ ];
            };
            systemd = lib.mkOption {
              type = lib.types.attrsOf lib.types.anything;
              default = { };
            };
          };
        }
        module
      ];
    }).config;
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
      adapter = adapters.${id};
      container = "agent-${name}";
      instance =
        args: backend:
        let
          selected =
            mkAgent
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
          module = moduleConfig (
            (builtins.head selected.imports) {
              config.virtualisation.oci-containers.backend = backend;
            }
          );
        };
      docker = instance { } "docker";
      podman = instance { } "podman";
      service = docker.module.systemd.services."docker-${container}";
      conflicting = moduleConfig (
        (adapter.build { inherit name; }) {
          config = {
            virtualisation.oci-containers.backend = "docker";
            virtualisation.oci-containers.containers.${container} = { };
            tentaflake.workspaceQuota.agents.${container}.enable = true;
          };
        }
      );
      failed = map (item: item.message) (lib.filter (item: !item.assertion) conflicting.assertions);
      refused =
        args:
        let
          result = instance args "docker";
        in
        rejects result || lib.any (item: !item.assertion) result.module.assertions;
    in
    assert (contract id adapter).identity.status == "scaffold";
    assert adapter.identity.version == version;
    assert adapter.artifact.kind == "unselected" && !adapter.artifact.reviewed;
    # Planned upstream facts are metadata only; the scaffold never runs them.
    assert adapter.command == command && adapter.capabilities == capabilities;
    assert adapter.research.supported == (id == "pi") && !(adapter.research.configure { }).valid;
    assert rejects (adapter.configuration.generate name { });
    assert adapter.execution.interface == "unverified" && !adapter.execution.automatic;
    assert adapter.evidence.vendorAcceptance == "pending";
    assert docker.record.adapter == id && !docker.record.runnable;
    assert docker.record.container == container;
    assert docker.record.unit == "docker-${container}.service";
    assert docker.record.uid == uid && docker.record.gid == uid;
    assert docker.record.stateDir == "/var/lib/tentaflake/agents/${name}";
    assert docker.record.workspace == "/var/lib/tentaflake/agents/${name}/workspace";
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
    assert lib.all (
      rule: !(lib.hasInfix "/var/lib/tentaflake/agents/${name}" rule)
    ) config.systemd.tmpfiles.rules;
    assert lib.all refused [
      { autoStart = true; }
      { settings.debug = true; }
      { stateDir = "/var/lib/tentaflake/agents/${name}/../outside"; }
      {
        image = "example.invalid/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      }
      { extraVolumes = [ "/host:/host:rw" ]; }
      { boardSocket = "/run/other/bridge.sock"; }
      { envFile = "/run/fixture.env"; }
    ];
    # Pi's source-qualified recovery blocker must not become a configurable
    # "safe mode": retry toggles and stable IDs do not fix close/resume replay.
    # These are admission tests, not simulated upstream crash acceptance.
    assert
      id != "pi"
      || lib.all (args: rejects (instance args "docker")) [
        { settings.retry.enabled = false; }
        { settings.recovery = "resume"; }
        { settings.requestId = "duplicate-operation"; }
        { settings.readOnly = true; }
        {
          command = [
            "node"
            "runner.mjs"
          ];
        }
        { researchSocket = "/run/research/relay.sock"; }
        { workerSocket = "/run/worker.sock"; }
        { extraVolumes = [ "/run/podman/podman.sock:/run/podman.sock:ro" ]; }
      ];
    assert
      id != "pi"
      || (
        lib.hasInfix "close/resume" service.script
        && lib.hasInfix "uncertain model request" service.script
        && docker.record == (instance { } "docker").record
        && docker.module == (instance { } "docker").module
        && !(service ? preStart)
        && !(service.serviceConfig ? StateDirectory)
        && !(service.serviceConfig ? LoadCredential)
      );
    assert
      id != "pi"
      || (
        !adapter.admitted { } { inherit name; }
        && (adapter.research.configure {
          server = {
            command = "/nix/store/fixture/bin/research-client";
            args = [
              "--socket"
              "/run/tentaflake-research/socket"
            ];
          };
        }).valid
        && !(adapter.research.configure {
          server = {
            command = "/bin/curl";
            args = [
              "--socket"
              "/run/foreign/socket"
            ];
          };
        }).valid
      );
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
  || !(adapters.${id}.metadata { name = "fixture"; } "docker").runnable
) (builtins.attrNames presets);
assert lib.length (lib.unique reserved) == lib.length reserved;
assert scaffold {
  id = "pi";
  name = "pilot";
  version = "1.4.0";
  uid = presets.pi.ownership.uid;
  command = [ ];
  capabilities = [ ];
};
assert scaffold {
  id = "openclaw";
  name = "assistant";
  version = "2026.9.7";
  uid = presets.openclaw.ownership.uid;
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
