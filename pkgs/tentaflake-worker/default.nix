{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "tentaflake-worker";
  version = "0.4.0";
  src = ../..;

  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = [ "-p=tentaflake-worker" ];
  cargoTestFlags = [ "-p=tentaflake-worker" ];

  meta = {
    description = "Fail-closed disposable tool-worker orchestrator for tentaflake";
    license = lib.licenses.mit;
    mainProgram = "tentaflake-worker";
    platforms = lib.platforms.linux;
  };
}
