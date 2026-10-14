{
  pkgs,
  eval,
  builders,
}:
let
  inherit (pkgs) lib;
  baseImages = import ../lib/baseImages.nix { inherit lib; };
  image = baseImages.image {
    id = "debian-13";
    system = pkgs.stdenv.hostPlatform.system;
  };
  empty = lib.genAttrs [
    "memoryBytes"
    "memorySwapBytes"
    "nanoCpus"
    "pids"
    "workspaceBytes"
    "stateBytes"
  ] (_: 0);
  template = {
    sourceInstance = "generic-template-source";
    imageId = "debian-13";
    principals = [ "operator" ];
    issuedActions = [
      "environments.read"
      "workspaces.read"
      "workspaces.retain"
    ];
  };
  fixture =
    definition: extra:
    eval (
      [
        (builders.mkAgent {
          adapter = "generic";
          name = "template-source";
          definition = {
            schemaVersion = 1;
            inherit image;
            command = [
              "sleep"
              "infinity"
            ];
            capabilities = [ ];
            resources = {
              cpus = "1";
              memory = "512m";
              memorySwap = "512m";
              pidsLimit = 128;
            };
          }
          // definition;
        })
        {
          tentaflake.workspaceQuota.agents.generic-template-source = {
            enable = true;
            workspace = "/var/lib/generic-template-source/workspace";
            sizeMiB = 128;
            state = {
              path = "/var/lib/generic-template-source/state";
              sizeMiB = 128;
            };
          };
          tentaflake.security.admission = {
            authorityEpoch = "template-fixture";
            physicalSpaceHeadroomBytes = 67108864;
            capacity = {
              memoryBytes = 2147483648;
              memorySwapBytes = 2147483648;
              nanoCpus = 2000000000;
              pids = 512;
              workspaceBytes = 4294967296;
              stateBytes = 4294967296;
            };
            hostReserve = empty;
            researchReserve = empty;
            maxActive = 2;
            creationTemplates.base = template;
          };
        }
      ]
      ++ extra
    );
  configured = fixture { } [ ];
  withoutTemplates = fixture { } [
    { tentaflake.security.admission.creationTemplates = lib.mkForce { }; }
  ];
  supervisor = configured.systemd.services."tentaflake-environment-create@";
  plan = builtins.fromJSON (
    builtins.unsafeDiscardStringContext configured.tentaflake.admissionManifest.text
  );
  projected = plan.creationTemplates.base;
  expectedImage = baseImages.catalog.images.debian-13;
  rejected =
    configuration:
    !(builtins.tryEval (builtins.deepSeq configuration.tentaflake.admissionManifest.text true)).success;
  badTemplate =
    changes:
    fixture { } [
      { tentaflake.security.admission.creationTemplates.base = lib.mkForce (template // changes); }
    ];
  ambiguous = fixture { } [ { tentaflake.security.admission.creationTemplates.other = template; } ];
  dynamic = import ../lib/quotaHelpers.nix { inherit pkgs lib; } {
    name = "template-test";
    kind = "state";
    bytes = 134217728;
    ownerUid = 10000;
    ownerGid = 10000;
    stateDirectories = [ "data" ];
    dynamic = true;
  };
in
assert lib.all (entry: entry.assertion) configured.assertions;
assert supervisor.wantedBy == [ ];
assert (supervisor.serviceConfig.User or "root") == "root";
assert supervisor.serviceConfig.Type == "oneshot";
assert !supervisor.serviceConfig.RemainAfterExit;
assert supervisor.serviceConfig.KillMode == "control-group";
assert supervisor.serviceConfig.Restart == "no";
assert supervisor.serviceConfig.TimeoutStartSec == 300;
assert supervisor.serviceConfig.TimeoutStopSec == 120;
assert
  supervisor.serviceConfig.ExecStart == lib.escapeShellArgs [
    plan.tools.cli
    "admission"
    "create-execute"
    (toString configured.tentaflake.admissionManifest)
    "%i"
  ];
assert !(withoutTemplates.systemd.services ? "tentaflake-environment-create@");
assert projected.sourceInstance == template.sourceInstance;
assert projected.sourceRevision == plan.instances.generic-template-source.revision;
assert
  projected.image == {
    id = "debian-13";
    inherit (expectedImage) distro release label;
    digest = expectedImage.platforms.${pkgs.stdenv.hostPlatform.system}.manifestDigest;
  };
assert projected.reference == image;
assert projected.principals == template.principals;
assert projected.issuedActions == template.issuedActions;
assert
  builtins.attrNames projected.helpers == [
    "state"
    "workspace"
  ];
assert lib.all (
  helpers:
  builtins.attrNames helpers == [
    "create"
    "mount"
    "owner"
    "unmount"
    "verify"
  ]
) (lib.attrValues projected.helpers);
assert rejected (badTemplate {
  sourceInstance = "missing";
});
assert rejected (badTemplate {
  imageId = "ubuntu-26.04";
});
assert rejected (badTemplate {
  principals = [
    "operator"
    "operator"
  ];
});
assert rejected (badTemplate {
  issuedActions = [ ];
});
assert rejected (fixture { capabilities = [ "model" ]; } [ ]);
assert rejected (fixture { resources.cpus = "0.5"; } [ ]);
assert rejected (
  fixture { } [
    { virtualisation.oci-containers.containers.generic-template-source.dependsOn = [ "source-seed" ]; }
  ]
);
assert lib.any (
  entry: !entry.assertion && lib.hasInfix "unique public configurations" entry.message
) ambiguous.assertions;
assert lib.hasInfix "^env-[0-9a-f]{32}$" dynamic.create.text;
assert lib.hasInfix ''if [ "$#" -ne 0 ]; then'' dynamic.create.text;
assert lib.hasInfix ''"/var/lib/tentaflake-environments/$slot/state"'' dynamic.create.text;
assert lib.hasInfix
  ''exec ${builtins.unsafeDiscardStringContext (toString dynamic.verify)} "$slot"''
  dynamic.create.text;
assert lib.hasInfix
  ''exec ${builtins.unsafeDiscardStringContext (toString dynamic.verify)} "$slot" --mounted''
  dynamic.mount.text;
assert lib.hasInfix ''"$workspace"/data'' dynamic.owner.text;
assert builtins.hasContext configured.tentaflake.admissionManifest.text;
assert plan.creationCatalog == builtins.unsafeDiscardStringContext "${../images/catalog.json}";
{
  creationTemplatePlan = configured.tentaflake.admissionManifest;
  creationTemplateHelpers = dynamic;
}
