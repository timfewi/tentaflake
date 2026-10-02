# Compatible typed entrypoint: retain the original formal arguments and defaults.
{ pkgs, lib }:
let
  constants = import ./constants.nix;
  mkAgent = import ./mkAgent.nix { inherit pkgs lib; };
in
{
  name,
  stateDir ? "/var/lib/hermes-${name}",
  user ? "hermes-${name}",
  group ? "hermes-${name}",
  uid ? null,
  gid ? null,
  image ? constants.hermesImage,
  allowMutableImage ? false,
  envFile ? null,
  agenixFile ? null,
  seedDir ? null,
  extraVolumes ? [ ],
  extraEnvironment ? { },
  cmd ? null,
  networkMode ? null,
  pidsLimit ? 512,
  autoStart ? true,
  createUser ? true,
  extraContainerConfig ? { },
  settings ? null,
  containerUid ? constants.containerUid,
  containerGid ? constants.containerGid,
  healDataDirs ? [ ],
  gitIdentity ? null,
  gitAutoPush ? null,
  dashboard ? null,
  services ? { },
  providerHealthcheck ? null,
}:
mkAgent {
  adapter = "hermes";
  inherit
    name
    stateDir
    user
    group
    uid
    gid
    image
    allowMutableImage
    envFile
    agenixFile
    seedDir
    extraVolumes
    extraEnvironment
    cmd
    networkMode
    pidsLimit
    autoStart
    createUser
    extraContainerConfig
    settings
    containerUid
    containerGid
    healDataDirs
    gitIdentity
    gitAutoPush
    dashboard
    services
    providerHealthcheck
    ;
}
