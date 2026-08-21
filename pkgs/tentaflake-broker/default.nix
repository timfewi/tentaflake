{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "tentaflake-broker";
  version = "0.4.0";
  src = ../..;

  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = [ "-p=tentaflake-broker" ];
  cargoTestFlags = [ "-p=tentaflake-broker" ];

  meta = {
    description = "Fail-closed credential and web-fetch policy broker for tentaflake";
    license = lib.licenses.mit;
    mainProgram = "tentaflake-broker";
    platforms = lib.platforms.linux;
  };
}
