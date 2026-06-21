<p align="center">
  <br/>
  <pre align="center">
                                                                         
                                                                         
                        ___      ___            _                ____    
                        `MM\     `M'           dM.              6MMMMb   
                         MMM\     M           ,MMb             8P    Y8  
                         M\MM\    M           d'YM.           6M      Mb 
                         M \MM\   M          ,P `Mb           MM      MM 
                         M  \MM\  M          d'  YM.          MM      MM 
                         M   \MM\ M         ,P   `Mb          MM      MM 
                         M    \MM\M MMMMMMM d'    YM. MMMMMMM MM      MM 
                         M     \MMM        ,MMMMMMMMb         YM      M9 
                         M      \MM        d'      YM.         8b    d8  
                        _M_      \M      _dM_     _dMM_         YMMMM9   
                                                                         
                                                                         
                                                                         
                          🐙  One NixOS brain · Many Hermes tentacles
  </pre>
  <p align="center">
    Declaratively deploy & manage multiple isolated Hermes AI agents
    <br/>
    on a single NixOS machine — each with its own secrets, skills, and personality.
  </p>
</p>

<p align="center">
  <a href="https://github.com/timfewi/nixos-agent-orchestration/actions"><img src="https://img.shields.io/github/actions/workflow/status/timfewi/nixos-agent-orchestration/check.yml?branch=main&style=flat-square" alt="CI status"/></a>
  <a href="https://github.com/timfewi/nixos-agent-orchestration/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="License: MIT"/></a>
  <a href="https://github.com/timfewi/nixos-agent-orchestration/blob/main/CONTRIBUTING.md"><img src="https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square" alt="PRs welcome"/></a>
  <a href="#"><img src="https://img.shields.io/badge/nixos-unstable-blue?style=flat-square&logo=nixos" alt="NixOS unstable"/></a>
  <a href="#"><img src="https://img.shields.io/badge/go-1.21+-00ADD8?style=flat-square&logo=go" alt="Go 1.21+"/></a>
  <br/>
  <a href="#-quick-start"><img src="https://img.shields.io/badge/🐙_Quickstart-%23262626?style=for-the-badge"/></a>
  <a href="#-features"><img src="https://img.shields.io/badge/🔧_Features-%23262626?style=for-the-badge"/></a>
  <a href="#-examples"><img src="https://img.shields.io/badge/📋_Examples-%23262626?style=for-the-badge"/></a>
  <a href="#-architecture"><img src="https://img.shields.io/badge/🏗️_Architecture-%23262626?style=for-the-badge"/></a>
</p>

---

## ✨ Features

| Capability | Description |
|---|---|
| **🧠 Multi-Agent** | Run any number of isolated Hermes agents on one machine |
| **🔒 Per-Agent Secrets** | Each agent gets its own API keys — no cross-contamination |
| **🐳 Docker Ephemeral** | Containers are stateless; personality + state live on mounted volumes |
| **⚙️ NixOS Declarative** | Everything defined in one flake — `nixos-rebuild switch` applies changes |
| **🔐 Agenix Support** | Encrypt secrets in-repo with age/agenix — decrypted at build time |
| **🎤 TTS Ready** | Built-in Piper TTS server (OpenAI-compatible `/v1/audio/speech`) |
| **📦 Live ISO** | Boot a USB with agents + TTS running out of the box — `nix build .#live-agent-iso` |
| **🔗 Tailscale** | Pre-configured Tailscale module for secure networking |

---

## 🚀 Quick Start

### 1. Clone

```bash
git clone https://github.com/timfewi/nixos-agent-orchestration
cd nixos-agent-orchestration
```

### 2. Customize

Edit `flake.nix` — set your hostname, admin user, timezone:

```nix
params = {
  hostName = "my-agent-box";
  adminUser = "alice";
  timeZone = "Europe/Vienna";
};
```

### 3. Define an agent

Create `my-agents.nix`:

```nix
{ mkHermesAgent }:
[
  (mkHermesAgent {
    name    = "coding";
    envFile = "/run/secrets/hermes-coding.env";
  })
]
```

### 4. Set up secrets

```bash
sudo mkdir -p /run/secrets
sudo cp hermes.env.example /run/secrets/hermes-coding.env
sudo chmod 600 /run/secrets/hermes-coding.env
sudo vi /run/secrets/hermes-coding.env
```

### 5. Build & deploy

```bash
nix flake check
sudo nixos-rebuild switch --flake .#agent-host
```

---

## 🏗 Architecture

```
                    🐙 Agent Orchestration
                One NixOS brain · Many tentacles

                          ╭─────────╮
                         ╱  🧠 NixOS ╲
                        │   Flake     │
                        │  (config)   │
                         ╲           ╱
                          ╰─────────╯
                    ┌───────┴────────┐
                    │                │
              ┌─────▼──────┐   ┌─────▼──────┐
              │  Tentacle  │   │  Tentacle  │    ...
              │  Agent A   │   │  Agent B   │
              │  (coding)  │   │ (research) │
              │            │   │            │
              │  🐳 Docker │   │  🐳 Docker │
              │  User:     │   │  User:     │
              │  hermes-A  │   │  hermes-B  │
              │  State:    │   │  State:    │
              │  /var/lib/ │   │  /var/lib/ │
              │  hermes-A  │   │  hermes-B  │
              │            │   │            │
              │  🔑 Key: A │   │  🔑 Key: B │
              │  📚 Skills │   │  📚 Skills │
              └────────────┘   └────────────┘

     ───────────────── Shared Services ─────────────────
    🎤 Piper TTS   🔗 Tailscale   🗄️ Docker   🔐 Agenix
    (port 5001)    (mesh VPN)     (runtime)    (secrets)

          ,---.
         ( @ @ )
          ).-.(
         '/|||\\`
           '|`
     🐙 little helper
```

### Key Design Decisions

| Decision | Rationale |
|---|---|
| **One container per agent** | Full isolation — no shared context, separate filesystems |
| **Host networking** | Agents reach local services (Piper, tailscale) via `localhost` |
| **SeedDir over :ro volumes** | Hermes can write learned skills; base files seed once, never overwrite |
| **Agenix or envFile** | Choose between encrypted-in-repo or plain-file secrets |
| **Template stays generic** | This repo is a template. Fork it, add your agents, keep your secrets. |

---

## 📋 Agent Configuration

### `mkHermesAgent` options

| Option | Type | Default | Description |
|---|---|---|---|
| `name` | `string` | *(required)* | Agent identifier (user, group, container, state dir) |
| `stateDir` | `string` | `/var/lib/hermes-<name>` | Isolated state directory |
| `image` | `string` | `nousresearch/hermes-agent:latest` | OCI container image |
| `envFile` | `path` | `null` | Path to `.env` file (injected via `--env-file`) |
| `agenixFile` | `path` | `null` | Path to agenix-decrypted env file |
| `seedDir` | `path` | `null` | Directory with SOUL.md, AGENTS.md, skills/ (seeded on first boot) |
| `settings` | `attrset` | `null` | Hermes `config.yaml` (model routing, TTS, STT, toolsets) |
| `extraVolumes` | `list` | `[]` | Extra `host:container:mode` mounts |
| `extraEnvironment` | `attrset` | `{}` | Extra env vars for the container |
| `extraContainerConfig` | `attrset` | `{}` | Extra Docker options (merged deep) |
| `autoStart` | `bool` | `true` | Auto-start with systemd |
| `cmd` | `list` | `["gateway" "run" "--replace"]` | Container entrypoint |

---

## 🔐 Secret Management

Two patterns — choose what fits your workflow:

### 1. `.env` file (simpler, local-only)

```nix
mkHermesAgent {
  name    = "my-agent";
  envFile = "/run/secrets/my-agent.env";
}
```

Place a plaintext `.env` file at the runtime path. **Never commit it to Git.**

### 2. Agenix (encrypted in Git)

Encrypt secrets as `.age` files — safe to commit, decrypted only at NixOS activation.

```nix
mkHermesAgent {
  name       = "my-agent";
  agenixFile = "/run/agenix/my-agent-env";
}
```

```bash
# Create encrypted secret
echo "OPENROUTER_API_KEY=sk-or-..." | agenix -e secrets/my-agent.env.age --stdin

# Rebuild — secrets never touch the Nix store
sudo nixos-rebuild switch --flake .#agent-host
```

📖 **Full guide:** [`docs/04-agenix-secrets.md`](docs/04-agenix-secrets.md) — setup, architecture, troubleshooting  
📋 **Template:** [`secrets.nix.example`](secrets.nix.example) — copy to `secrets.nix` to start

Both patterns keep secrets **out of the Nix store** and **never in Nix evaluation**.

---

## 📦 Available Modules

| Module | What it configures |
|---|---|
| `modules/default.nix` | Module aggregator (imports all others) |
| `boot.nix` | systemd-boot, EFI, kernel params |
| `locale.nix` | Timezone, locale, console keymap |
| `networking.nix` | Hostname, nftables firewall, NetworkManager |
| `users.nix` | Admin user (wheel + networkmanager groups) |
| `nix-settings.nix` | Flakes, auto-GC, trusted-users, substituters |
| `packages.nix` | curl, git, jq, tmux, vim, and more |
| `hardening.nix` | Sysctl hardening, AppArmor, journald limits |
| `tailscale.nix` | Tailscale with SSH + tag:auto (optional) |
| `piper-tts-server.nix` | Local TTS via Piper (OpenAI-compatible API) |
| `hermes-firstboot.nix` | USB env detection + first-boot TUI wizard (live ISO) |

---

## 🎯 Examples

### Two agents — coding assistant + web researcher

```nix
{ mkHermesAgent }:
[
  (mkHermesAgent {
    name    = "coding";
    envFile = "/run/secrets/hermes-coding.env";
    settings = {
      model.default = "openrouter/anthropic/claude-sonnet-4";
      model.provider = "openrouter";
      terminal.backend = "docker";
      web.backend = "disabled";
      toolsets = [ "terminal" "memory" "file" "skills" ];
    };
  })
  (mkHermesAgent {
    name    = "research";
    envFile = "/run/secrets/hermes-research.env";
    settings = {
      model.default = "openrouter/deepseek/deepseek-v4-flash";
      model.provider = "openrouter";
      web.backend = "firecrawl";
      toolsets = [ "terminal" "web" "memory" "file" "skills" ];
    };
  })
]
```

### Live ISO (demo or deployment)

```bash
# Build a bootable USB with Hermes agents + Piper TTS
nix build .#live-agent-iso

# Write to USB (replace /dev/sdX with your device)
sudo cp result/iso/nixos-agent-orchestration-live.iso /dev/sdX

# Boot it — enter API keys in the TUI wizard, agents start.
# Or plug a USB labeled HERMES_ENV with .env files for zero-touch.
```

---

## 🛠 Commands

```bash
# Validate flake
nix flake check

# Build live ISO (Hermes agents + TTS out of the box)
nix build .#live-agent-iso

# Dry-run activation
sudo nixos-rebuild dry-activate --flake .#agent-host

# Build and deploy
sudo nixos-rebuild switch --flake .#agent-host

# Rollback
sudo nixos-rebuild switch --rollback

# List running agents
docker ps --filter "name=hermes-"

# View agent logs
docker logs hermes-coding

# Chat with an agent
docker exec -it hermes-coding hermes chat
```

---

## 🤝 Contributing

This is a community template — contributions welcome!

1. Fork the repo
2. Create a feature branch (`git checkout -b feat/amazing`)
3. Commit your changes (`git commit -m "feat: add amazing thing"`)
4. Push (`git push origin feat/amazing`)
5. Open a Pull Request

Please keep the template **generic** — no domain-specific code belongs here. That goes in your fork.

---

## 📄 License

<p align="center">
  MIT — see <a href="https://github.com/timfewi/nixos-agent-orchestration/blob/main/LICENSE">LICENSE</a><br/>
  <sub>Piper voice models distributed under their respective MIT licenses.</sub>
</p>
