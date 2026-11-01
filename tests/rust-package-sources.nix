{ self, pkgs }:
let
  members = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.members;
  names = map builtins.baseNameOf members;
  # Cargo must parse foreign member manifests even when their sources are
  # deliberately absent. An implicit-only target breaks every isolated package.
  explicitTargets =
    member:
    let
      manifest = builtins.fromTOML (builtins.readFile (../. + "/${member}/Cargo.toml"));
    in
    (manifest ? lib && manifest.lib ? path)
    || ((manifest.bin or [ ]) != [ ] && lib.all (target: target ? path) manifest.bin);
  # Every current workspace member ships a binary; broker and worker also need
  # the named shared activity library, but never unrelated runtime sources.
  binaryNames = names;
  packages = self.packages.${pkgs.stdenv.hostPlatform.system};
  isolated =
    name:
    let
      source = packages.${name}.src;
      expectedSources = [
        name
      ]
      ++ lib.optional (builtins.elem name [
        "tentaflake-broker"
        "tentaflake-worker"
      ]) "tentaflake-activity";
    in
    builtins.pathExists (source + "/crates/${name}/src/main.rs")
    && lib.all (member: builtins.pathExists (source + "/${member}/Cargo.toml")) members
    && lib.all (
      other:
      (builtins.pathExists (source + "/crates/${other}/src")) == builtins.elem other expectedSources
    ) names;
  inherit (pkgs) lib;
in
assert lib.all explicitTargets members;
assert builtins.elem "tentaflake-integrations" names;
assert
  (builtins.fromTOML (builtins.readFile ../crates/tentaflake-integrations/Cargo.toml)).lib.path
  == "src/lib.rs";
assert lib.all isolated binaryNames;
assert builtins.pathExists (packages.tentaflake-cli.src + "/adapters/catalog.json");
assert builtins.pathExists (packages.tentaflake-cli.src + "/schema/system-v1.json");
assert builtins.pathExists (packages.tentaflake-cli.src + "/schema/system-v1-fixtures.json");
assert !(builtins.pathExists (packages.tentaflake-broker.src + "/adapters/catalog.json"));
assert !(builtins.pathExists (packages.tentaflake-worker.src + "/adapters/catalog.json"));
assert packages.tentaflake-worker-image.imageTag == packages.tentaflake-worker.version;
pkgs.runCommand "tentaflake-rust-package-sources" { } "touch $out"
