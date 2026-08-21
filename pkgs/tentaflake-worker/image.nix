{
  dockerTools,
  bash,
  coreutils,
  cargo,
  rustc,
  gcc,
  gnumake,
  gitMinimal,
  findutils,
  gnugrep,
  gnused,
  gnutar,
  gzip,
  pkg-config,
}:

dockerTools.buildLayeredImage {
  name = "tentaflake-worker";
  tag = "0.4.0";
  maxLayers = 100;

  contents = [
    bash
    coreutils
    cargo
    rustc
    gcc
    gnumake
    gitMinimal
    findutils
    gnugrep
    gnused
    gnutar
    gzip
    pkg-config
  ];

  config = {
    Cmd = [ "${bash}/bin/bash" ];
    Env = [
      "CARGO_NET_OFFLINE=true"
      "PATH=/bin"
    ];
    WorkingDir = "/workspace";
  };
}
