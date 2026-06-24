---
name: handle-the-host
description: Connect to and operate a tentaflake-built machine via Tailscale SSH. Use when remoting into a deployed agent-host for maintenance, debugging, rebuilds, or inspection.
version: 1.0.0
---

# Handle the Host — Remote Operations Guide

## Overview

Tentaflake deploys NixOS with Tailscale pre-configured. Every target machine must join your tailnet. You connect, inspect, rebuild, and debug entirely through Tailscale SSH — no open ports, no public IP required.

## How to Connect

```bash
tailscale ssh <user>@<hostname>
```

Replace `<user>` with the admin username set during installation (default: `admin`). Replace `<hostname>` with the hostname chosen during installation.

> **Note:** `tailscale ssh` does **not** support the `-t` flag. For commands requiring a pseudo-TTY (e.g., `sudo` prompts), use raw SSH through Tailscale instead:
> ```bash
> ssh -t <user>@<hostname> "sudo <command>"
> ```

## First Connection Checklist

1. **Verify Tailscale status** on the target:
   ```bash
   tailscale ssh <user>@<hostname>
   tailscale status
   ```

2. **Check that Hermes agents are running:**
   ```bash
   docker ps
   ```

3. **Inspect NixOS config** (always in `/etc/nixos/`):
   ```bash
   ls /etc/nixos/
   ```

## Common Operations

### Rebuild the System

```bash
cd /etc/nixos
sudo nixos-rebuild switch --flake .#<hostname>
```

Or use the `rebuild` alias if configured:
```bash
rebuild
```

### Rollback

```bash
sudo nixos-rebuild switch --rollback
```

### Inspect System State

```bash
# Nix store usage
sudo nix store gc --print-dead

# Running services
systemctl list-units --type=service --state=running

# Journal logs (Hermes agent)
journalctl -u hermes-agent -f
```

### Container Operations

```bash
# List Hermes agent containers
docker ps

# Restart a specific agent
sudo systemctl restart docker-hermes-<agent-name>

# Inspect container logs
docker logs hermes-<agent-name>
```

## Passwordless Sudo for Tailscale & NixOS

By default, the tentaflake-hardening module enables `sudo.wheelNeedsPassword = true`. For smooth remote operations you may want passwordless sudo for Tailscale and NixOS rebuild commands.

### Option A: NixOS Module (if using host-monitor or custom module)

In your host configuration:

```nix
security.sudo.extraRules = [
  {
    groups = [ "wheel" ];
    commands = [
      {
        command = "${pkgs.tailscale}/bin/tailscale";
        options = [ "NOPASSWD" ];
      }
      {
        command = "${pkgs.nixos-rebuild}/bin/nixos-rebuild";
        options = [ "NOPASSWD" ];
      }
      {
        command = "/run/current-system/sw/bin/nixos-rebuild";
        options = [ "NOPASSWD" ];
      }
      {
        command = "${pkgs.nix}/bin/nix";
        options = [ "NOPASSWD" ];
      }
    ];
  }
];
```

Add this to a dedicated `modules/sudo.nix` or inline in `configuration.nix`.

### Option B: Manual (for existing deployments)

SSH in and edit sudoers:

```bash
ssh -t <user>@<hostname> "echo '%wheel ALL=(ALL) NOPASSWD: /run/current-system/sw/bin/tailscale, /run/current-system/sw/bin/nixos-rebuild' | sudo EDITOR='tee' visudo -f /etc/sudoers.d/tailscale-nixos"
```

## Tailscale Serve (Exposing Services)

Use `tailscale serve` to expose local services to your tailnet:

```bash
# Serve a local port
sudo tailscale serve --bg 8080

# Serve HTTPS
sudo tailscale serve --bg --https=443 localhost:8080

# List active serves
tailscale serve status
```

## Verification

After connecting, confirm the machine is healthy:

```bash
tailscale ssh <user>@<hostname>
hostname               # Should match the installed hostname
tailscale status        # Should show the machine joined to tailnet
systemctl status        # Should show running state
docker ps               # Should list agent containers
```

## Pitfalls

- **`tailscale ssh` fails silently:** Ensure the target machine is online and has Tailscale running. Check `tailscale status` from your admin machine.
- **`nixos-rebuild` needs sudo:** All NixOS operations require root. Use the NOPASSWD rules above or authenticate via `ssh -t`.
- **Config at `/etc/nixos/`:** Always work from `/etc/nixos/`. The installed system uses its own flake there — NOT the installer ISO's repo.
- **`tailscale ssh` limits:** Pseudo-TTY (-t) is not supported. Use raw SSH with `-t` for interactive sudo prompts.
- **Firewall:** Tentaflake enables nftables with a restrictive default-deny posture. Tailscale's WireGuard interface is allowed by default. For extra services, open ports in your host config.
