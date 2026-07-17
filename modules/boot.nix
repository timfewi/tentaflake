{ config, lib, ... }:
let
  cfg = config.tentaflake.boot;
in
lib.mkIf cfg.enable {
  boot.loader.systemd-boot.enable = true;
  # Block editing kernel cmdline at the boot menu (init=/bin/sh root shell).
  boot.loader.systemd-boot.editor = false;
  boot.loader.efi.canTouchEfiVariables = true;
}
