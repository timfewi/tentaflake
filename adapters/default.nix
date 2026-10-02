# Explicit lazy imports: discovering names does not force any runtime.
{ pkgs, lib }:
{
  hermes = import ./hermes.nix { inherit pkgs lib; };
  zeroclaw = import ./zeroclaw.nix { inherit pkgs lib; };
  openclaw = import ./openclaw.nix { inherit pkgs lib; };
}
