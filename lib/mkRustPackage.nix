{ lib, rustPlatform }:

{
  pname,
  mainProgram ? pname,
  postInstall ? "",
}:

let
  crate = ../crates + "/${pname}";
  inherit (builtins.fromTOML (builtins.readFile (crate + "/Cargo.toml"))) package;
  inherit (builtins.fromTOML (builtins.readFile ../Cargo.toml)) workspace;
in
rustPlatform.buildRustPackage {
  inherit pname postInstall;
  inherit (workspace.package) version;
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions (
      [
        ../Cargo.toml
        ../Cargo.lock
        # Cargo resolves the whole workspace, but only builds this member.
        (lib.fileset.fileFilter (file: file.name == "Cargo.toml") ../crates)
        crate
      ]
      ++ lib.optionals (pname == "tentaflake-cli") [
        ../adapters/catalog.json
        ../images/catalog.json
        # TLS client regressions consume the authoritative wire exchanges.
        ../api/v1/fixtures.json
      ]
      ++ lib.optional (lib.elem pname [
        "tentaflake-cli"
        "tentaflake-worker"
      ]) ../crates/tentaflake-admission
    );
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "-p=${pname}" ];
  cargoTestFlags = [
    "-p=${pname}"
  ]
  ++ lib.optional (lib.elem pname [
    "tentaflake-cli"
    "tentaflake-worker"
  ]) "-p=tentaflake-admission";

  meta = {
    inherit (package) description;
    inherit mainProgram;
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
