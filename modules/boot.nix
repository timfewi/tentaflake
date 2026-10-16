{ config, lib, ... }:
let
  cfg = config.tentaflake.boot;
in
lib.mkIf cfg.enable {
  # GRUB on the layout of the disk image (flake.nix): the BIOS boot partition
  # and the master boot record for BIOS, the removable path on the ESP for
  # UEFI, so the image boots with whichever firmware the provider uses. A VPS
  # console belongs to the provider account, which can attach a rescue system
  # to the disk anyway, so the boot menu has no password.
  boot.loader.grub = {
    enable = true;
    # The virtio disk of a QEMU guest.
    device = "/dev/vda";
    efiSupport = true;
    efiInstallAsRemovable = true;
    # Older generations stay selectable for a rollback; their kernels live on the ESP.
    configurationLimit = 10;
  };
}
