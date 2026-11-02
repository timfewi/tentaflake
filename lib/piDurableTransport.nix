{ lib, pkgs }:
let
  runtimes = import ./gvisorRuntimes.nix { inherit lib pkgs; };
  inherit (runtimes.pi-durable) runtime path;
  at =
    config: keys: fallback:
    lib.attrByPath keys fallback config;
in
rec {
  inherit runtime path;
  name = "runsc-pi-durable";
  version = "runsc-pi-durable-v1";
  containerSupported =
    container:
    lib.elem (container.podman.user or null) [
      null
      "root"
    ]
    && !(lib.any (
      option:
      lib.hasPrefix "--userns" option
      || lib.hasPrefix "--uidmap" option
      || lib.hasPrefix "--gidmap" option
    ) ((container.extraOptions or [ ]) ++ (container.preRunExtraOptions or [ ])));
  # This establishes configured transport, not runtime acceptance. Only the
  # canonical wrapper may grant bind+connect; caller flags cannot emulate it.
  configurationSupported =
    config:
    let
      backend = at config [ "virtualisation" "oci-containers" "backend" ] "";
      docker = at config [ "virtualisation" "docker" "daemon" "settings" ] { };
      selected = docker.runtimes.runsc-pi-durable or { };
      engine = at config [ "virtualisation" "containers" "containersConf" "settings" "engine" ] { };
    in
    at config [ "tentaflake" "piDurableRuntime" "enable" ] false
    && at config [ "tentaflake" "piDurableRuntime" "transport" ] null == "runsc-pi-durable-v1"
    && runtimes.configurationSupported config
    && (
      if backend == "docker" then
        selected == {
          inherit path;
        }
        && (docker."userns-remap" or "") == ""
        && (docker."default-runtime" or "runc") != "runsc-pi-durable"
        && !(at config [ "virtualisation" "docker" "rootless" "enable" ] false)
        && at config [ "virtualisation" "docker" "extraOptions" ] "" == ""
      else if backend == "podman" then
        (engine.runtimes.runsc-pi-durable or [ ]) == [ path ]
        && (engine.runtime or "crun") != "runsc-pi-durable"
        &&
          (at config [
            "virtualisation"
            "containers"
            "containersConf"
            "settings"
            "containers"
            "userns"
          ] "host") == "host"
      else
        false
    );
  # Keep declaration construction independent of derived systemd services.
  # Full runtime assertions and capability publication must use this stronger
  # predicate; otherwise services -> declarations -> services forms a cycle.
  supported =
    config:
    configurationSupported config
    && (
      at config [ "virtualisation" "oci-containers" "backend" ] "" != "docker"
      || at config [ "systemd" "services" "docker" "serviceConfig" "User" ] "root" == "root"
    );
}
