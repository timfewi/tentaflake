# Tentaflake — Install Guide

Build the ISO and write it to a USB stick. Two options depending on what you
want:

| ISO | Command | What it does |
|-----|---------|-------------|
| **Live Agent ISO** | `nix build .#live-agent-iso` | Boots straight into Hermes agents + Piper TTS in RAM. **No install.** Pull the USB and every trace is gone. |
| **Installer ISO** | `nix build .#installer-iso` | Boots into a TUI wizard that formats a disk and installs Tentaflake permanently. |

---

## 1. Build the ISO

From the repo root:

```bash
# Live agent ISO (run in RAM, no disk touch)
nix build .#live-agent-iso

# OR installer ISO (permanent install to disk)
nix build .#installer-iso
```

Or use the convenience script:

```bash
./scripts/build-iso.sh              # live-agent-iso (default)
./scripts/build-iso.sh installer    # installer-iso
```

When done, the ISO is at:

| ISO | Path |
|-----|------|
| Live | `result/iso/tentaflake-live.iso` |
| Installer | `result/iso/tentaflake.iso` |

---

## 2. Identify your USB stick

```bash
lsblk
```

Look for your USB device by size and label — typically `/dev/sda`, `/dev/sdb`,
or `/dev/nvme0n1` (rare for USB). **Triple-check** — the wrong device gets
wiped.

```
NAME   MAJ:MIN RM   SIZE RO TYPE MOUNTPOINTS
sda      8:0    1  28.7G  0 disk            ← your USB (28 GB)
└─sda1   8:1    1  28.7G  0 part
nvme0n1 259:0    0 931.5G  0 disk           ← your system disk (DO NOT TOUCH)
```

Unmount any mounted partitions first:

```bash
sudo umount /dev/sdX1 2>/dev/null || true
```

---

## 3. Write the ISO to USB

> ⚠️ **Destructive.** This wipes everything on the target device.

```bash
sudo dd if=result/iso/tentaflake-live.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

Replace `/dev/sdX` with your actual USB device from step 2 (e.g. `/dev/sdb`).
Same command works for both ISOs — just swap the `.iso` filename.

The image is a **UEFI + legacy-BIOS hybrid**, so it boots on both modern and
older machines.

---

## 4. Verify the write

```bash
sync
sudo blkid /dev/sdX
```

Expect a filesystem type like `iso9660` or similar — confirms the ISO landed.

---

## 5. Boot from USB

1. Insert the USB into the target machine.
2. Enter the boot menu (usually `F10`/`F12`/`Esc` during POST).
3. Select the USB device (UEFI or legacy-BIOS, either works).
4. Save and boot.

### Live ISO boots into a firstboot wizard

Auto-logs into TTY1 and prompts for API keys. Enter at minimum an OpenRouter
key. Piper TTS is already serving on `http://localhost:5001/v1`.

> **Unattended boot?** Put `.env` files on a second USB labeled `TENTAFLAKE_ENV`
> (legacy `HERMES_ENV` also accepted) and the wizard is skipped automatically. See
> [README.md](../README.md#skip-the-wizard-unattended-boot) for details.

### Installer ISO boots into a TUI installer

A `dialog`-based wizard walks through partitioning, hardware config, and
`nixos-install`. Takes 10–15 minutes.

---

## Next Steps

Once booted (live) or after install + reboot, follow the post-install guide:

➡️ [01-quickstart.md](01-quickstart.md) — set up agents, configure providers, start chatting
