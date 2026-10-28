{
  lib,
  python3,
  makeWrapper,
  stdenvNoCC,
}:
stdenvNoCC.mkDerivation {
  pname = "tentaflake-host-api";
  version = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).workspace.package.version;
  src = ../../host-api;
  nativeBuildInputs = [
    makeWrapper
    python3
  ];
  dontBuild = true;
  doCheck = true;
  checkPhase = ''
    runHook preCheck
    python3 -m unittest discover -s tests -p 'test_*.py'
    runHook postCheck
  '';
  installPhase = ''
    runHook preInstall
    install -Dm644 tentaflake_host_api.py "$out/lib/tentaflake_host_api.py"
    makeWrapper ${python3}/bin/python3 "$out/bin/tentaflake-host-api" \
      --add-flags "$out/lib/tentaflake_host_api.py"
    runHook postInstall
  '';
  meta = {
    description = "Optional bounded Unix-socket operator API for tentaflake";
    mainProgram = "tentaflake-host-api";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
