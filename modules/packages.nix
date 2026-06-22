{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake;
in
lib.mkIf cfg.packages.enable {
  environment.systemPackages = with pkgs; [
    curl
    git
  ];
}
