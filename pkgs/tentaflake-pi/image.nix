{
  lib,
  dockerTools,
  runCommand,
  coreutils,
  gnutar,
  gzip,
  jq,
  cacert,
  runtimePackage,
}:
let
  tag = builtins.hashString "sha256" (toString runtimePackage);
  imageSelector = "tentaflake-pi:${tag}";
  archive = dockerTools.buildLayeredImage {
    name = "tentaflake-pi";
    inherit tag;
    maxLayers = 80;
    contents = [
      runtimePackage
      cacert
    ];
    extraCommands = ''
      mkdir -p state workspace etc/tentaflake run/tentaflake-pi tmp
      chmod 1777 tmp
    '';
    config = {
      User = "10100:10100";
      Cmd = [
        "/bin/tentaflake-pi"
        "/etc/tentaflake/pi.json"
      ];
      WorkingDir = "/workspace";
      Env = [
        "PATH=/bin"
        "HOME=/state"
        "SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt"
      ];
    };
  };
in
runCommand "tentaflake-pi-image-bundle"
  {
    nativeBuildInputs = [
      coreutils
      gnutar
      gzip
      jq
    ];
    passthru = {
      inherit runtimePackage imageSelector;
      imageVerificationVersion = 1;
      imageArchive = archive;
    };
    meta = {
      description = "Local pinned Pi image with exact after-load config ID proof";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
    };
  }
  ''
    set -euo pipefail
    mkdir "$out"
    tar --extract --gzip --to-stdout --file ${archive} manifest.json > manifest.json
    config="$(jq -er --arg image ${lib.escapeShellArg imageSelector} '
      if type == "array" and length == 1 and .[0].RepoTags == [$image]
      then .[0].Config else error("unexpected image manifest") end
    ' manifest.json)"
    if [[ ! "$config" =~ ^([0-9a-f]{64}[.]json|blobs/sha256/[0-9a-f]{64})$ ]]; then
      echo "invalid image config path" >&2
      exit 1
    fi
    tar --extract --gzip --to-stdout --file ${archive} "$config" > config.json
    sha256sum config.json > digest
    read -r digest _ < digest
    printf 'sha256:%s\n' "$digest" > "$out/image-id"
    ln -s ${archive} "$out/image.tar.gz"
  ''
