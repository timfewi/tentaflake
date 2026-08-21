{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "tentaflake-cli";
  version = "0.4.0";
  src = ../..;

  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = [ "-p=tentaflake-cli" ];
  cargoTestFlags = [ "-p=tentaflake-cli" ];

  postInstall = ''
    ln -s tentaflake $out/bin/tentaflake-status
    ln -s tentaflake $out/bin/hermes
  '';

  meta = {
    description = "Operator CLI for tentaflake agent hosts";
    license = lib.licenses.mit;
    mainProgram = "tentaflake";
    platforms = lib.platforms.linux;
  };
}
