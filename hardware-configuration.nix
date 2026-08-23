# Minimal hardware config for flake evaluation.
# Replace with output of:
#   nixos-generate-config --show-hardware-config
_: {
  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-label/boot";
    fsType = "vfat";
  };
}
