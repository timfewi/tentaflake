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

let
  inherit (builtins.fromTOML (builtins.readFile ../../crates/tentaflake-worker/Cargo.toml)) package;
  inherit (builtins.fromTOML (builtins.readFile ../../Cargo.toml)) workspace;
in
dockerTools.buildLayeredImage {
  inherit (package) name;
  tag = workspace.package.version;
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
