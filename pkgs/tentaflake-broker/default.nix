{
  lib,
  rustPlatform,
}:

import ../../lib/mkRustPackage.nix { inherit lib rustPlatform; } {
  pname = "tentaflake-broker";
  localCrates = [ ../../crates/tentaflake-activity ];
}
