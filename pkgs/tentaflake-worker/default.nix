{
  lib,
  rustPlatform,
}:

import ../../lib/mkRustPackage.nix { inherit lib rustPlatform; } {
  pname = "tentaflake-worker";
  localCrates = [ ../../crates/tentaflake-activity ];
}
