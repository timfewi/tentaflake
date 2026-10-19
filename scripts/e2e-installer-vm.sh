#!/usr/bin/env bash
# Launch the installer ISO or its installed system in a disposable UEFI VM.
# The guest receives only one repository-owned QCOW2 file, never a host block
# device. Installation still erases that virtual disk after TUI confirmation.
set -euo pipefail

mode=${1:-install}
case "$mode" in
  install | boot) ;;
  *)
    echo "usage: $0 {install|boot}" >&2
    exit 2
    ;;
esac

repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo_dir"
state_dir=${TENTAFLAKE_E2E_DIR:-/var/tmp/tentaflake-e2e-$(id -un)}

case "$state_dir" in
  /*) ;;
  *)
    echo "error: TENTAFLAKE_E2E_DIR must be an absolute path" >&2
    exit 1
    ;;
esac

if [ -L "$state_dir" ]; then
  echo "error: refusing symlinked E2E state directory: $state_dir" >&2
  exit 1
fi

install -d -m 0700 "$state_dir"
state_dir=$(realpath --canonicalize-existing "$state_dir")

if [ "$(stat -c %u "$state_dir")" -ne "$(id -u)" ]; then
  echo "error: E2E state directory is not owned by the current user: $state_dir" >&2
  exit 1
fi

nix_package() {
  local attribute=$1
  nix build \
    --no-link \
    --print-out-paths \
    --impure \
    --expr "
      let
        flake = builtins.getFlake (toString ./.);
      in
      flake.inputs.nixpkgs.legacyPackages.x86_64-linux.${attribute}
    "
}

qemu_root=$(nix_package qemu_kvm)
ovmf_root=$(nix_package OVMF.fd)
qemu_system="$qemu_root/bin/qemu-system-x86_64"
qemu_img="$qemu_root/bin/qemu-img"
ovmf_code="$ovmf_root/FV/OVMF_CODE.fd"
ovmf_vars="$state_dir/OVMF_VARS.fd"
disk="$state_dir/tentaflake.qcow2"

if [ ! -e "$ovmf_vars" ]; then
  install -m 0600 "$ovmf_root/FV/OVMF_VARS.fd" "$ovmf_vars"
elif [ -L "$ovmf_vars" ] || [ ! -f "$ovmf_vars" ]; then
  echo "error: invalid UEFI variable store: $ovmf_vars" >&2
  exit 1
fi

if [ "$mode" = install ]; then
  "$repo_dir/scripts/build-iso.sh" installer
  iso=$(readlink --canonicalize-existing "$repo_dir/result/iso/tentaflake.iso")

  if [ ! -e "$disk" ]; then
    "$qemu_img" create -f qcow2 "$disk" 32G
  fi
elif [ ! -e "$disk" ]; then
  echo "error: no installed VM disk at $disk" >&2
  echo "run 'just e2e-installer' first" >&2
  exit 1
fi

if [ -L "$disk" ] || [ ! -f "$disk" ]; then
  echo "error: invalid VM disk: $disk" >&2
  exit 1
fi

if [ "$(stat -c %u "$disk")" -ne "$(id -u)" ]; then
  echo "error: VM disk is not owned by the current user: $disk" >&2
  exit 1
fi

if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
  acceleration=kvm
  cpu=host
else
  acceleration=tcg
  cpu=max
  echo "warning: /dev/kvm is unavailable; the VM will use slower TCG emulation" >&2
fi

qemu_args=(
  -name "tentaflake-e2e"
  -machine "q35,accel=$acceleration"
  -cpu "$cpu"
  -smp 4
  -m 4096
  -drive "if=pflash,unit=0,format=raw,readonly=on,file=$ovmf_code"
  -drive "if=pflash,unit=1,format=raw,file=$ovmf_vars"
  -drive "file=$disk,if=virtio,format=qcow2"
  -nic "user,model=virtio-net-pci"
)

if [ "$mode" = install ]; then
  qemu_args+=(
    -drive "file=$iso,media=cdrom,readonly=on,format=raw"
    -boot "order=c,once=d,menu=on"
  )
  echo "==> Starting the installer ISO in a disposable UEFI VM"
  echo "    The installer can erase only: $disk"
else
  qemu_args+=(
    -boot "order=c,menu=on"
  )
  echo "==> Booting the installed disposable UEFI VM"
  echo "    Disk: $disk"
fi

if [ "${TENTAFLAKE_E2E_DRY_RUN:-0}" = 1 ]; then
  printf '==> QEMU command:'
  printf ' %q' "$qemu_system" "${qemu_args[@]}"
  printf '\n'
  exit 0
fi

exec "$qemu_system" "${qemu_args[@]}"
