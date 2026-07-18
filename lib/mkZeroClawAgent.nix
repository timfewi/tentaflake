{ pkgs, lib }:

let
  constants = import ./constants.nix;
  pinnedImage = import ./pinnedImage.nix { inherit lib; };
in

{
  name,
  agenixFile,
  # Digest-pinned (lib/constants.nix). Overrides must be digest-pinned too — a
  # bare tag is rejected at eval time, see lib/pinnedImage.nix.
  image ? constants.zeroclawImage,
  # Escape hatch for locally-built images that have no registry digest.
  allowMutableImage ? false,
  stateDir ? "/var/lib/zeroclaw-${name}",
  seedDir ? null,
  gatewayPort ? 42617,
  hostPort,
  servePort,
  autoStart ? true,
  # Max number of processes in the container (--pids-limit). null = unlimited.
  pidsLimit ? 512,
  settings ? { },
  extraEnvironment ? { },
  extraVolumes ? [ ],
}:

{ config, ... }:
let
  containerName = "zeroclaw-${name}";
  serviceName = "${config.virtualisation.oci-containers.backend}-${containerName}.service";
  toml = pkgs.formats.toml { };
  configFile = toml.generate "${containerName}-config.toml" settings;
in
{
  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 65534 65534 -"
    "d ${stateDir}/.zeroclaw 0700 65534 65534 -"
    "d ${stateDir}/.zeroclaw/data 0700 65534 65534 -"
    "d ${stateDir}/data 0700 65534 65534 -"
  ];

  systemd.services =
    lib.optionalAttrs (seedDir != null) {
      "seed-${containerName}" = {
        description = "Seed workspace for ZeroClaw agent ${name}";
        wantedBy = [ serviceName ];
        before = [ serviceName ];
        after = [ "systemd-tmpfiles-setup.service" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${pkgs.bash}/bin/bash -c 'cp -rn --no-preserve=mode,ownership ${seedDir}/. ${stateDir}/.zeroclaw/data/ 2>/dev/null || true; chown -R 65534:65534 ${stateDir}; chmod -R u+rwX ${stateDir}'";
        };
      };
    }
    // {
      "${containerName}-tailscale-serve" = {
        description = "Tailscale Serve for ZeroClaw agent ${name}";
        wantedBy = [ "multi-user.target" ];
        after = [
          "tailscaled.service"
          serviceName
        ];
        wants = [
          "tailscaled.service"
          serviceName
        ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${pkgs.tailscale}/bin/tailscale serve --bg --https=${toString servePort} 127.0.0.1:${toString hostPort}";
        };
      };
    };

  virtualisation.oci-containers.containers.${containerName} = {
    inherit autoStart;
    image = pinnedImage name allowMutableImage image;
    cmd = [ "daemon" ];
    volumes = [
      "${stateDir}:/zeroclaw-data:rw"
      "${configFile}:/zeroclaw-data/.zeroclaw/config.toml:ro"
    ]
    ++ extraVolumes;
    ports = [ "127.0.0.1:${toString hostPort}:${toString gatewayPort}" ];
    environment = {
      HOME = "/zeroclaw-data";
      ZEROCLAW_DATA_DIR = "/zeroclaw-data/data";
      ZEROCLAW_gateway__port = toString gatewayPort;
    }
    // extraEnvironment;
    extraOptions = [
      "--env-file=${agenixFile}"
      "--security-opt=no-new-privileges:true"
    ]
    ++ lib.optional (pidsLimit != null) "--pids-limit=${toString pidsLimit}";
  };
}
