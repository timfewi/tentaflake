{
  lib,
  stdenvNoCC,
  nodejs_24,
  makeWrapper,
  piRuntime,
}:
assert lib.versionAtLeast nodejs_24.version "24";
assert piRuntime.sourceRevision == "35dfa757378bd7a8570670e99c2687bdd1039a06";
stdenvNoCC.mkDerivation {
  pname = "tentaflake-pi";
  version = "0.1.0";
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
    mkdir -p "$out/lib/tentaflake-pi" "$out/bin"
    cp -r *.mjs package.json UPSTREAM-LICENSE test "$out/lib/tentaflake-pi/"
    ln -s ${piRuntime.nodeModules} "$out/lib/tentaflake-pi/node_modules"
    makeWrapper ${nodejs_24}/bin/node "$out/bin/tentaflake-pi" \
      --add-flags "$out/lib/tentaflake-pi/runner.mjs"
    makeWrapper ${nodejs_24}/bin/node "$out/bin/tentaflake-pi-test" \
      --add-flags "--test $out/lib/tentaflake-pi/test/ledger.test.mjs $out/lib/tentaflake-pi/test/mediation.test.mjs $out/lib/tentaflake-pi/test/sdk-recovery.test.mjs"
    runHook postInstall
  '';
  passthru = {
    upstreamRevision = piRuntime.sourceRevision;
    inherit piRuntime;
  };
  meta = {
    description = "Quarantine-first isolated Pi Durable runner";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "tentaflake-pi";
  };
}
