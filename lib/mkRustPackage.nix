{ lib, rustPlatform }:

{
  pname,
  description,
  mainProgram ? pname,
  postInstall ? "",
}:

rustPlatform.buildRustPackage {
  inherit pname postInstall;
  version = "0.4.0";
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "-p=${pname}" ];
  cargoTestFlags = [ "-p=${pname}" ];

  meta = {
    inherit description mainProgram;
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
