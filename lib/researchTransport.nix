{ lib, pkgs }:
let
  runtimes = import ./gvisorRuntimes.nix { inherit lib pkgs; };
  inherit (runtimes.research) runtime path;
  at =
    config: keys: fallback:
    lib.attrByPath keys fallback config;
in
{
  inherit runtime path;
  name = "runsc-research";
  selected =
    backend: container: expected:
    let
      options = lib.filter (option: lib.hasPrefix "--runtime" option) (container.extraOptions or [ ]);
      preRun = container.preRunExtraOptions or [ ];
    in
    if backend == "docker" then
      options == [ "--runtime=${expected}" ] && preRun == [ ]
    else if backend == "podman" then
      options == [ ]
      &&
        preRun == [
          "--runtime"
          expected
        ]
    else
      false;
  # Registration is not a grant: only explicitly Research-enabled containers
  # may select this runtime. Never make it the engine-wide default.
  supported =
    config:
    let
      backend = at config [ "virtualisation" "oci-containers" "backend" ] "";
      docker = at config [ "virtualisation" "docker" "daemon" "settings" ] { };
      engine = at config [ "virtualisation" "containers" "containersConf" "settings" "engine" ] { };
    in
    at config [ "tentaflake" "security" "profile" ] "dev" != "dev"
    && at config [ "tentaflake" "research" "agents" ] { } != { }
    && runtimes.configurationSupported config
    && (
      if backend == "docker" then
        (docker.runtimes.runsc-research or { }) == {
          inherit path;
        }
        && (docker."default-runtime" or "runc") != "runsc-research"
        && at config [ "virtualisation" "docker" "extraOptions" ] "" == ""
      else if backend == "podman" then
        (engine.runtimes.runsc-research or [ ]) == [ path ]
        && (engine.runtime or "crun") != "runsc-research"
      else
        false
    );
}
