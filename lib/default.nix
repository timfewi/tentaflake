{ pkgs, lib }:
{
  mkAgent = import ./mkAgent.nix { inherit pkgs lib; };
  runtimeCatalog = import ./runtimeCatalog.nix;
  adapters = import ../adapters { inherit pkgs lib; };
  mkHermesAgent = import ./mkHermesAgent.nix { inherit pkgs lib; };
  mkZeroClawAgent = import ./mkZeroClawAgent.nix { inherit pkgs lib; };
  agentsFromData = import ./agentsFromData.nix { inherit pkgs lib; };
  constants = import ./constants.nix;
}
