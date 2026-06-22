{ config, lib, ... }:
let
  cfg = config.tentaflake.boot;
in
lib.mkIf cfg.enable {
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
}
