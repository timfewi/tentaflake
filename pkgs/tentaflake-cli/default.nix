{
  lib,
  rustPlatform,
}:

import ../../lib/mkRustPackage.nix { inherit lib rustPlatform; } {
  pname = "tentaflake-cli";
  description = "Operator CLI for tentaflake agent hosts";
  mainProgram = "tentaflake";

  postInstall = ''
    ln -s tentaflake $out/bin/tentaflake-status
    ln -s tentaflake $out/bin/hermes
  '';
}
