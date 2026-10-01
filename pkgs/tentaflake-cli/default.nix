{
  lib,
  rustPlatform,
}:

import ../../lib/mkRustPackage.nix { inherit lib rustPlatform; } {
  pname = "tentaflake-cli";
  mainProgram = "tentaflake";

  postInstall = ''
    ln -s tentaflake $out/bin/tentaflake-status
    ln -s tentaflake $out/bin/hermes
  '';
}
