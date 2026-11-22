{
  lib,
  python3,
  rustPlatform,
}:
let
  broker = import ../../lib/mkRustPackage.nix { inherit lib rustPlatform; } {
    pname = "tentaflake-broker";
    localCrates = [ ../../crates/tentaflake-activity ];
  };
in
broker.overrideAttrs {
  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    ${python3}/bin/python3 ${../../tests/broker-provider-grants.py} "$out/bin/tentaflake-broker"
    runHook postInstallCheck
  '';
}
