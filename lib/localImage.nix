# Describe a locally built Docker archive without importing build results at eval.
# This establishes content identity, not registry or publisher provenance.
{ pkgs, archive }:
let
  archivePath = if builtins.isPath archive then "${archive}" else toString archive;
in
assert pkgs.lib.assertMsg (
  builtins.hasContext archivePath
  &&
    builtins.match "/nix/store/[0-9abcdfghijklmnpqrsvwxyz]{32}-[A-Za-z0-9+._?=-]+(/[A-Za-z0-9+._?=-]+)*" archivePath
    != null
  && pkgs.lib.all (part: part != "." && part != "..") (pkgs.lib.splitString "/" archivePath)
) "tentaflake localImage requires an immutable Nix store archive with dependency context.";
pkgs.runCommand "tentaflake-local-image.json" { } ''
  ${pkgs.python3}/bin/python3 ${../scripts/describe_docker_archive.py} \
    ${pkgs.lib.escapeShellArg archivePath} > "$out"
''
