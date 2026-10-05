{ self, pkgs }:
let
  members = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.members;
  names = map builtins.baseNameOf members;
  packages = self.packages.${pkgs.stdenv.hostPlatform.system};
  isolated =
    name:
    let
      source = packages.${name}.src;
    in
    builtins.pathExists (source + "/crates/${name}/src/main.rs")
    && lib.all (member: builtins.pathExists (source + "/${member}/Cargo.toml")) members
    && lib.all (other: other == name || !(builtins.pathExists (source + "/crates/${other}/src"))) names;
  inherit (pkgs) lib;
in
assert lib.all isolated names;
assert builtins.pathExists (packages.tentaflake-cli.src + "/adapters/catalog.json");
assert !(builtins.pathExists (packages.tentaflake-broker.src + "/adapters/catalog.json"));
assert !(builtins.pathExists (packages.tentaflake-worker.src + "/adapters/catalog.json"));
assert packages.tentaflake-worker-image.imageTag == packages.tentaflake-worker.version;
pkgs.runCommand "tentaflake-rust-package-sources" { } "touch $out"
