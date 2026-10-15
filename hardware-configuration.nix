# The VPS guest: QEMU with virtio devices, booted from the disk image the
# flake builds (`nix build .#image-raw`). The root file system grows to the
# disk the provider attaches; NetworkManager configures the network by DHCP.
{ modulesPath, ... }:
{
  imports = [ (modulesPath + "/profiles/qemu-guest.nix") ];

  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
    autoResize = true;
  };
  fileSystems."/boot" = {
    device = "/dev/disk/by-label/ESP";
    fsType = "vfat";
    options = [
      "fmask=0077"
      "dmask=0077"
    ];
  };
  boot.growPartition = true;

  # Kernel messages and a login prompt on the graphical and the serial console.
  boot.kernelParams = [
    "console=ttyS0,115200"
    "console=tty0"
  ];
}
