# Tentaflake install guide

Tentaflake ships one bootable image: the installer ISO. The former live-agent
ISO and firstboot agent environment were removed from the core.

## Build

From the repository root:

```bash
nix build .#installer-iso
```

Or use the wrapper:

```bash
./scripts/build-iso.sh installer
```

The image is written below `result/iso/`.

## Test in a disposable VM

From the contributor shell, the repository can build the ISO, resolve its
pinned QEMU and OVMF tools, create one 32 GiB sparse QCOW2 disk, and start the
interactive UEFI installer:

```bash
just e2e-installer
```

The VM receives no host block device. The installer can erase only
`/var/tmp/tentaflake-e2e-<user>/tentaflake.qcow2`, and still asks for
confirmation in its TUI. The first boot uses the ISO once and then prefers the
installed disk. To boot the installed VM again without the ISO:

```bash
just e2e-run-vm
```

Set `TENTAFLAKE_E2E_DIR` to an absolute path to keep this disposable VM state
elsewhere. The scripts deliberately provide no automatic reset or deletion.

## Resolve the USB device

List block devices and identify the USB drive by size and transport:

```bash
lsblk -o NAME,SIZE,TYPE,TRAN,MOUNTPOINTS
```

Do not infer the target from an example device name. The next step destroys
all data on the selected device.

Unmount its mounted partitions, substituting the exact partition:

```bash
sudo umount /dev/sdX1
```

## Write the image

Resolve the exact ISO filename first:

```bash
find result/iso -maxdepth 1 -name '*.iso'
```

Then write it to the verified whole device, not a partition:

```bash
sudo dd \
  if=result/iso/tentaflake.iso \
  of=/dev/sdX \
  bs=4M \
  status=progress \
  oflag=sync
```

Replace `/dev/sdX` only after checking it again with `lsblk`.

## Boot and install

Boot the target machine from the USB device. The installer auto-logs into its
local console and starts the `dialog`-based installer. It asks before
partitioning and runs `nixos-install` only after the selected disk is shown.

Disk formatting and installation are runtime mutations. A successful ISO build
does not authorize either operation.

After installation and reboot, continue with
[the quick start](01-quickstart.md).

The installer environment itself uses the dev profile because it runs no
agents. The generated installed host uses the default `balanced` profile. Old
direct-secret/port agent definitions therefore require the explicit migration
described in [security profiles](10-security-profiles.md); installation never
silently downgrades the target to dev.
