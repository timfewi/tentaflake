# Only the VM gets this test harness. The ordinary CLI package and command parser
# expose no operation seeder or synthetic supervisor identity.
{ pkgs }:
let
  cli = pkgs.callPackage ../../pkgs/tentaflake-cli { };
in
cli.overrideAttrs (old: {
  pname = "tentaflake-creation-harness";
  doCheck = false;
  buildPhase = ''
    runHook preBuild
    cargo test --offline --locked --release -j "$NIX_BUILD_CORES" -p tentaflake-cli --bin tentaflake \
      --no-run --message-format=json > creation-artifacts.json
    ${pkgs.python3}/bin/python3 - <<'PY'
    import json
    import pathlib

    executables = []
    with open("creation-artifacts.json", encoding="utf-8") as stream:
        for line in stream:
            record = json.loads(line)
            if (record.get("reason") == "compiler-artifact"
                    and record.get("target", {}).get("name") == "tentaflake"
                    and record.get("target", {}).get("kind") == ["bin"]
                    and record.get("profile", {}).get("test") is True
                    and record.get("executable")):
                executables.append(record["executable"])
    if len(executables) != 1:
        raise SystemExit("expected one CLI test harness")
    executable = pathlib.Path(executables[0]).resolve(strict=True)
    if not executable.is_relative_to(pathlib.Path.cwd()) or not executable.is_file():
        raise SystemExit("unexpected CLI test harness location")
    pathlib.Path("creation-harness-path").write_text(str(executable), encoding="utf-8")
    PY
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    install -D -m 0755 "$(cat creation-harness-path)" "$out/bin/tentaflake-creation-fixture"
    runHook postInstall
  '';
  postInstall = "";
  meta = old.meta // {
    mainProgram = "tentaflake-creation-fixture";
    description = "Test-only accepted creation fixture for the NixOS VM";
  };
})
