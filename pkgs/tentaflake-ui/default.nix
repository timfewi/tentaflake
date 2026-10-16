{
  lib,
  stdenvNoCC,
  bun,
}:

let
  inherit (builtins.fromTOML (builtins.readFile ../../Cargo.toml)) workspace;
  inherit (workspace.package) version;

  # The runtime dependencies exactly as ui/bun.lock pins them, fetched by Bun
  # in a fixed-output derivation. Update the hash when the lockfile changes.
  nodeModules = stdenvNoCC.mkDerivation {
    pname = "tentaflake-ui-node-modules";
    inherit version;
    src = lib.fileset.toSource {
      root = ../../ui;
      fileset = lib.fileset.unions [
        ../../ui/package.json
        ../../ui/bun.lock
      ];
    };
    nativeBuildInputs = [ bun ];
    dontConfigure = true;
    dontFixup = true;
    buildPhase = ''
      runHook preBuild
      export HOME=$TMPDIR
      bun install --frozen-lockfile --production --ignore-scripts --no-progress
      rm -rf node_modules/.cache node_modules/.bin
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mv node_modules $out
      runHook postInstall
    '';
    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-TsSbD8HEGmYbb0hgG9EUu0UxludbGuid6tI1qAKl4bw=";
  };
in
# The built single-page app (ui/dist) the control service serves.
stdenvNoCC.mkDerivation {
  pname = "tentaflake-ui";
  inherit version;
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../ui/package.json
      ../../ui/tsconfig.json
      ../../ui/index.html
      ../../ui/src
      # ui/src/client.ts uses the API types and constants of the control service.
      ../../control/src/api.ts
    ];
  };

  nativeBuildInputs = [ bun ];
  dontConfigure = true;
  buildPhase = ''
    runHook preBuild
    export HOME=$TMPDIR
    cd ui
    cp -r --no-preserve=mode ${nodeModules} node_modules
    bun run build
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    cp -r dist $out
    runHook postInstall
  '';

  passthru = { inherit nodeModules; };

  meta = {
    description = "Tentaflake web UI";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
