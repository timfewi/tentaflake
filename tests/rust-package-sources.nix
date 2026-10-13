{ self, pkgs }:
let
  members = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.members;
  names = map builtins.baseNameOf members;
  binaries = map builtins.baseNameOf (
    lib.filter (
      member: (builtins.fromTOML (builtins.readFile (../. + "/${member}/Cargo.toml"))) ? bin
    ) members
  );
  packages = self.packages.${pkgs.stdenv.hostPlatform.system};
  isolated =
    name:
    let
      source = packages.${name}.src;
      sources = [ name ] ++ lib.optional (name == "tentaflake-cli") "tentaflake-admission";
    in
    builtins.pathExists (source + "/crates/${name}/src/main.rs")
    && lib.all (member: builtins.pathExists (source + "/${member}/Cargo.toml")) members
    && lib.all (
      other: builtins.pathExists (source + "/crates/${other}/src") == lib.elem other sources
    ) names;
  inherit (pkgs) lib;
in
assert lib.all isolated binaries;
assert builtins.pathExists (
  packages.tentaflake-cli.src + "/crates/tentaflake-admission/src/lib.rs"
);
assert builtins.pathExists (packages.tentaflake-cli.src + "/adapters/catalog.json");
assert !(builtins.pathExists (packages.tentaflake-broker.src + "/adapters/catalog.json"));
assert !(builtins.pathExists (packages.tentaflake-worker.src + "/adapters/catalog.json"));
assert packages.tentaflake-worker-image.imageTag == packages.tentaflake-worker.version;
pkgs.runCommand "tentaflake-rust-package-sources" { } "touch $out"
