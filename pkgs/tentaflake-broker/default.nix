{
  lib,
  rustPlatform,
}:

import ../../lib/mkRustPackage.nix { inherit lib rustPlatform; } {
  pname = "tentaflake-broker";
  description = "Fail-closed credential and web-fetch policy broker for tentaflake";
}
