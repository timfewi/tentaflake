{
  lib,
  stdenvNoCC,
  nodejs_24,
  makeWrapper,
  piDurableRuntime,
}:
assert lib.versionAtLeast nodejs_24.version "24";
assert piDurableRuntime.sourceRevision == "b4afdddf44e059c67ccf5097e5317d913521b470";
stdenvNoCC.mkDerivation {
  pname = "tentaflake-pi-durable";
  inherit ((builtins.fromTOML (builtins.readFile ../../Cargo.toml)).workspace.package) version;
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      (lib.fileset.fileFilter (file: lib.hasSuffix ".mjs" file.name) ./.)
      ./package.json
      ./UPSTREAM-LICENSE
      ./test
    ];
  };
  nativeBuildInputs = [ makeWrapper ];
  dontBuild = true;
  installPhase = ''
    runHook preInstall
    mkdir -p "$out/lib/tentaflake-pi-durable" "$out/bin"
    cp -r *.mjs package.json UPSTREAM-LICENSE test "$out/lib/tentaflake-pi-durable/"
    ln -s ${piDurableRuntime.nodeModules} "$out/lib/tentaflake-pi-durable/node_modules"
    makeWrapper ${nodejs_24}/bin/node "$out/bin/tentaflake-pi-durable" \
      --add-flags "$out/lib/tentaflake-pi-durable/runner.mjs"
    makeWrapper ${nodejs_24}/bin/node "$out/bin/tentaflake-pi-durable-test" \
      --add-flags "--test $out/lib/tentaflake-pi-durable/test/ledger.test.mjs $out/lib/tentaflake-pi-durable/test/mediation.test.mjs $out/lib/tentaflake-pi-durable/test/sdk-recovery.test.mjs $out/lib/tentaflake-pi-durable/test/sdk-success.test.mjs $out/lib/tentaflake-pi-durable/test/provider-grants.test.mjs"
    makeWrapper ${nodejs_24}/bin/node "$out/bin/tentaflake-pi-durable-tools-test" \
      --add-flags "--test --test-name-pattern=tool.selection $out/lib/tentaflake-pi-durable/test/sdk-recovery.test.mjs"
    runHook postInstall
  '';
  passthru = {
    upstreamRevision = piDurableRuntime.sourceRevision;
    inherit piDurableRuntime;
  };
  meta = {
    description = "Quarantine-first isolated Pi Durable runner";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "tentaflake-pi-durable";
  };
}
