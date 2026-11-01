# Synthetic engine/archive contract, not acceptance evidence for a real daemon.
{ pkgs }:
let
  inherit (pkgs) lib;
  selector = "tentaflake-proof:local";
  config = "{\"architecture\":\"amd64\",\"config\":{},\"rootfs\":{\"type\":\"layers\",\"diff_ids\":[]}}\n";
  expected = "sha256:${builtins.hashString "sha256" config}";
  manifestId = "sha256:${lib.concatStrings (lib.replicate 64 "a")}";
  fixtures = pkgs.runCommand "local-image-proof-fixtures" { } ''
    mkdir -p "$out"
    ${pkgs.python3}/bin/python3 - "$out" <<'PY'
    import io
    import json
    import pathlib
    import sys
    import tarfile

    root = pathlib.Path(sys.argv[1])
    config = ${builtins.toJSON config}
    name = "blobs/sha256/" + "b" * 64
    entry = {"Config": name, "RepoTags": [${builtins.toJSON selector}], "Layers": []}

    def archive(case, manifest, payload=config.encode(), member_name=name, duplicate=False, link=False):
        with tarfile.open(root / (case + ".tar"), "w", format=tarfile.USTAR_FORMAT) as tar:
            def add(path, data):
                info = tarfile.TarInfo(path)
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
            add("manifest.json", manifest)
            if duplicate:
                add("manifest.json", manifest)
            if link:
                info = tarfile.TarInfo(member_name)
                info.type = tarfile.SYMTYPE
                info.linkname = "/etc/passwd"
                tar.addfile(info)
            else:
                add(member_name, payload)

    valid = json.dumps([entry]).encode()
    archive("modern", valid)
    legacy_name = "c" * 64 + ".json"
    archive("legacy-export", json.dumps([{**entry, "Config": legacy_name}]).encode(), member_name=legacy_name)
    (root / "malformed-archive.tar").write_bytes(b"not a tar archive")
    archive("changed", valid, b"{}\n")
    archive("wrong-tag", json.dumps([{**entry, "RepoTags": ["wrong:tag"]}]).encode())
    archive("multi-image", json.dumps([entry, entry]).encode())
    archive("traversal", json.dumps([{**entry, "Config": "../config.json"}]).encode(), member_name="../config.json")
    archive("malformed", b"{not JSON")
    archive("oversized-config", valid, b"x" * (4 * 1024 * 1024 + 1))
    archive("oversized-manifest", b" " * (64 * 1024 + 1))
    archive("duplicate-manifest", valid, duplicate=True)
    archive("symlink-config", valid, link=True)
    archive("missing-config", valid, member_name="absent.json")
    with tarfile.open(root / "duplicate-config.tar", "w", format=tarfile.USTAR_FORMAT) as tar:
        for path, data in [("manifest.json", valid), (name, config.encode()), (name, config.encode())]:
            info = tarfile.TarInfo(path)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    with tarfile.open(root / "null-duplicate-manifest.tar", "w", format=tarfile.USTAR_FORMAT) as tar:
        for path, data in [("manifest.json", b"null"), ("manifest.json", valid), (name, config.encode())]:
            info = tarfile.TarInfo(path)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    PY
  '';
  bundle =
    (pkgs.runCommand "local-image-proof-bundle" { } ''
      mkdir -p "$out"
      printf '%s\n' ${lib.escapeShellArg expected} > "$out/image-id"
    '')
    // {
      imageVerificationVersion = 1;
      imageSelector = selector;
    };
  cases = [
    {
      name = "classic";
      success = true;
      direct = true;
    }
    {
      name = "bare-classic";
      success = true;
      direct = true;
    }
    {
      name = "modern";
      success = true;
    }
    {
      name = "legacy-export";
      success = true;
    }
    { name = "changed"; }
    { name = "wrong-tag"; }
    { name = "multi-image"; }
    { name = "traversal"; }
    { name = "malformed"; }
    { name = "malformed-archive"; }
    { name = "oversized-config"; }
    { name = "oversized-manifest"; }
    { name = "duplicate-manifest"; }
    { name = "null-duplicate-manifest"; }
    { name = "duplicate-config"; }
    { name = "symlink-config"; }
    { name = "missing-config"; }
    { name = "export-failure"; }
    {
      name = "malformed-id";
      direct = true;
    }
    {
      name = "inspect-failure";
      direct = true;
    }
    {
      name = "podman-modern";
      backend = "podman";
      direct = true;
    }
    {
      name = "podman-classic";
      backend = "podman";
      success = true;
      direct = true;
    }
  ];
  runCase =
    case:
    let
      backend = case.backend or "docker";
      actual =
        if
          builtins.elem case.name [
            "classic"
            "podman-classic"
          ]
        then
          expected
        else if case.name == "bare-classic" then
          lib.removePrefix "sha256:" expected
        else if case.name == "malformed-id" then
          "not-an-image-id"
        else
          manifestId;
      engine = pkgs.writeShellScriptBin backend ''
        set -euo pipefail
        if [[ "$#" == 5 && "$1" == image && "$2" == inspect && "$3" == --format && "$4" == '{{.Id}}' && "$5" == ${lib.escapeShellArg selector} ]]; then
          printf 'inspect\n' >> "$ENGINE_LOG"
          ${lib.optionalString (case.name == "inspect-failure") "exit 1"}
          printf '%s\n' ${lib.escapeShellArg actual}
        elif [[ "$#" == 3 && "$1" == image && "$2" == save && "$3" == ${lib.escapeShellArg selector} ]]; then
          printf 'save\n' >> "$ENGINE_LOG"
          ${
            if case.name == "export-failure" then
              ''
                printf 'partial export'
                exit 1
              ''
            else
              ''
                ${pkgs.coreutils}/bin/cat ${fixtures}/${case.name}.tar
              ''
          }
        else
          printf 'FORBIDDEN\n' >> "$ENGINE_LOG"
          exit 99
        fi
      '';
      verifier = import ../lib/verifyLocalImage.nix {
        inherit lib backend bundle;
        image = selector;
        pkgs = pkgs // {
          ${backend} = engine;
        };
      };
    in
    ''
      export ENGINE_LOG="$PWD/${case.name}.log"
      : > "$ENGINE_LOG"
      if ${verifier}; then
        ${if case.success or false then ":" else "echo 'unexpected acceptance: ${case.name}' >&2; exit 1"}
      else
        ${if case.success or false then "echo 'unexpected refusal: ${case.name}' >&2; exit 1" else ":"}
      fi
      printf 'inspect\n${lib.optionalString (!(case.direct or false)) "save\\n"}' > expected.log
      cmp expected.log "$ENGINE_LOG"
      test -z "$(find "$TMPDIR" -mindepth 1 -print -quit)"
    '';
in
pkgs.runCommand "local-image-proof" { } ''
  mkdir proof-tmp
  export TMPDIR="$PWD/proof-tmp"
  ${lib.concatMapStringsSep "\n" runCase cases}
  touch "$out"
''
