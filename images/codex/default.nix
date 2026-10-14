# Native reference image only: packaging is not provider/runtime acceptance.
# The Generic command supplies exactly one explicitly selected broker model.
{ pkgs, mkLocalImage }:
let
  inherit (pkgs) lib;
  artifact = import ./artifact.nix { inherit pkgs; };
  package = pkgs.runCommand "codex-reference-package-0.162.1" { } ''
    mkdir "$out"
    ${pkgs.gnutar}/bin/tar --extract --use-compress-program=${pkgs.gzip}/bin/gzip --file ${artifact} \
      --directory "$out" --no-same-owner --no-same-permissions
    test -x "$out/bin/codex"
    test -x "$out/bin/codex-code-mode-host"
    test -x "$out/codex-path/rg"
    test -x "$out/codex-resources/bwrap"
    test -x "$out/codex-resources/zsh/bin/zsh"
    test -x "$out/codex-resources/voice/bin/codex-voice-host"
    ${pkgs.python3}/bin/python3 - "$out" <<'PY'
    import hashlib
    import json
    import sys
    from pathlib import Path
    root = Path(sys.argv[1])
    assert json.loads((root / "codex-package.json").read_text()) == {
        "layoutVersion": 1,
        "version": "0.162.1",
        "target": "x86_64-unknown-linux-musl",
        "variant": "codex",
        "entrypoint": "bin/codex",
        "resourcesDir": "codex-resources",
        "pathDir": "codex-path",
    }, "unexpected Codex package layout"
    manifest = json.loads((root / "codex-resources/voice/manifest.json").read_text())
    assert manifest["schemaVersion"] == 1
    assert manifest["appVersion"] == "0.162.1"
    assert manifest["appTarget"] == "x86_64-unknown-linux-musl"
    assert manifest["voiceTarget"] == "x86_64-unknown-linux-gnu"
    for name, expected in manifest["sha256"].items():
        relative = Path(name)
        assert not relative.is_absolute() and ".." not in relative.parts
        with (root / relative).open("rb") as source:
            assert hashlib.file_digest(source, "sha256").hexdigest() == expected, "Codex package content changed"
    PY
  '';
  entrypoint = pkgs.writeTextFile {
    name = "tentaflake-codex-entrypoint";
    destination = "/bin/tentaflake-codex";
    executable = true;
    text = lib.replaceStrings [ "@python@" "@codex@" ] [ "${pkgs.python3}" "${package}" ] (
      builtins.readFile ./entrypoint.py
    );
  };
  ownership = (import ../../lib/runtimeCatalog.nix).presets.generic.ownership;
  root = pkgs.buildEnv {
    name = "tentaflake-codex-root";
    paths = [
      package
      entrypoint
      pkgs.bashInteractive
      pkgs.coreutils
      pkgs.gitMinimal
      pkgs.python3
    ];
    pathsToLink = [ "/bin" ];
    # These are image paths, not assumptions about or changes to the host.
    # Preserve the upstream binaries and bundled license/manifest bytes.
    postBuild = ''
      mkdir -p "$out/lib64" "$out/lib" "$out/usr/bin" "$out/etc"
      ln -s ${package}/codex-resources/bwrap "$out/bin/bwrap"
      ln -s ${pkgs.glibc}/lib/ld-linux-x86-64.so.2 "$out/lib64/ld-linux-x86-64.so.2"
      ln -s ${lib.getLib pkgs.ncurses}/lib/libtinfo.so.6 "$out/lib/libtinfo.so.6"
      ln -s ${pkgs.coreutils}/bin/env "$out/usr/bin/env"
      printf 'root:x:0:0:root:/root:/bin/sh\ncodex:x:${toString ownership.uid}:${toString ownership.gid}:Codex:/state:/bin/bash\n' > "$out/etc/passwd"
      printf 'root:x:0:\ncodex:x:${toString ownership.gid}:\n' > "$out/etc/group"
    '';
  };
  archive = pkgs.dockerTools.buildLayeredImage {
    name = "tentaflake-codex-reference";
    tag = "0.162.1";
    contents = [ root ];
    config = {
      Entrypoint = [ "/bin/tentaflake-codex" ];
      WorkingDir = "/workspace";
      User = "${toString ownership.uid}:${toString ownership.gid}";
      Env = [
        "PATH=/bin:${package}/codex-path"
        "HOME=/state"
        "CODEX_HOME=/state/.codex"
        "SHELL=/bin/bash"
        "LD_LIBRARY_PATH=${
          lib.makeLibraryPath [
            pkgs.glibc
            pkgs.ncurses
          ]
        }:${package}/codex-resources/voice/lib"
      ];
    };
  };
in
{
  inherit package archive;
  descriptor = mkLocalImage archive;
}
