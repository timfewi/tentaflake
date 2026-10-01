{
  lib,
  rustPlatform,
}:

import ../../lib/mkRustPackage.nix { inherit lib rustPlatform; } {
  pname = "tentaflake-worker";
  description = "Fail-closed disposable tool-worker orchestrator for tentaflake";
}
