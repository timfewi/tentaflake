# Discover adapter files without forcing their runtime implementations.
{ pkgs, lib }:
let
  entries = builtins.readDir ./.;
  files = builtins.filter (
    file:
    entries.${file} == "regular"
    && file != "default.nix"
    && builtins.match "[a-z0-9][a-z0-9-]*\\.nix" file != null
  ) (builtins.attrNames entries);
in
builtins.listToAttrs (
  map (file: {
    name = builtins.elemAt (builtins.match "(.*)\\.nix" file) 0;
    value = import (./. + "/${file}") { inherit pkgs lib; };
  }) files
)
