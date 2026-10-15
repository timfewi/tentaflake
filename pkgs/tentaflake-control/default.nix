{
  lib,
  stdenvNoCC,
  bun,
  makeBinaryWrapper,
}:

let
  inherit (builtins.fromTOML (builtins.readFile ../../Cargo.toml)) workspace;
in
# Bun runs the TypeScript sources directly; the service has no runtime dependencies.
stdenvNoCC.mkDerivation {
  pname = "tentaflake-control";
  inherit (workspace.package) version;
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../control/package.json
      ../../control/src
      # control/src/environments.ts imports the runtime image's base configuration.
      ../../images/runtime/opencode.json
    ];
  };

  nativeBuildInputs = [ makeBinaryWrapper ];
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    runHook preInstall
    mkdir -p $out/share
    cp -r . $out/share/tentaflake-control
    makeBinaryWrapper ${lib.getExe bun} $out/bin/tentaflake-control \
      --add-flags "--no-install run $out/share/tentaflake-control/control/src/main.ts" \
      --set BUN_RUNTIME_TRANSPILER_CACHE_PATH 0
    runHook postInstall
  '';

  meta = {
    description = "Tentaflake control service: users, keys, environments and the session proxy";
    mainProgram = "tentaflake-control";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
