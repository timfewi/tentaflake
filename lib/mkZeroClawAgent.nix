# Compatible typed entrypoint: retain the original formal arguments and defaults.
{ pkgs, lib }:
let
  constants = import ./constants.nix;
  mkAgent = import ./mkAgent.nix { inherit pkgs lib; };
in

{
  name,
  agenixFile ? null,
  image ? constants.zeroclawImage,
  allowMutableImage ? false,
  stateDir ? "/var/lib/zeroclaw-${name}",
  seedDir ? null,
  gatewayPort ? 42617,
  hostPort ? null,
  servePort ? null,
  autoStart ? true,
  pidsLimit ? 512,
  settings ? { },
  extraEnvironment ? { },
  extraVolumes ? [ ],
  extraContainerConfig ? { },
}:
mkAgent {
  adapter = "zeroclaw";
  inherit
    name
    agenixFile
    image
    allowMutableImage
    stateDir
    seedDir
    gatewayPort
    hostPort
    servePort
    autoStart
    pidsLimit
    settings
    extraEnvironment
    extraVolumes
    extraContainerConfig
    ;
}
