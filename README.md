# 🪼 Tentaflake — NixOS Flake Template for Hermes Multi-Agent

> Deploy isolated Hermes AI agents on a single headless machine.
> One NixOS brain · Many Hermes tentacles.

> **What is Hermes?** [Hermes](https://github.com/NousResearch/hermes-agent) is an open-source AI agent daemon from Nous Research — like a personal AI assistant that runs on your own hardware. It connects to LLM providers (OpenRouter, Anthropic, OpenAI), runs tools (terminal, web search, file access), and can be customized with skills and personality. Tentaflake gives you a turnkey way to run one or many Hermes agents on a dedicated machine.

<p align="center">
  <a href="https://tentaflake.dev"><img src="https://img.shields.io/badge/tentaflake.dev-00d4ff?style=flat-square&labelColor=0a1628" alt="tentaflake.dev"/></a>
  <a href="https://github.com/timfewi/tentaflake/actions"><img src="https://img.shields.io/github/actions/workflow/status/timfewi/tentaflake/check.yml?branch=main&style=flat-square" alt="CI"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="MIT"/></a>
  <img src="https://img.shields.io/badge/nixos-unstable-blue?style=flat-square&logo=nixos" alt="NixOS"/>
</p>

<p align="center">
  <br/>
  <pre align="center">


                                                \#pB&  NBBL /dBp
                                                 \NBBL  NBBZdBB> 1w
                                             |#gggpBBBISLBBBBB% IBB&/
                                             ]3BBBBBBBBBBBBBBp QBBB/
                            ~                """""1111    *NBBCBBB/                 w
                            /              ,,aaag5BBE       9tBBBbbbBBL            IT
                         3dBBBpL           ]BBBBBBBFL       zdBBBBBB##F          3dBBBpp
                        ?B*\,,TBL          1S$EQBBFdpc     IdBB>                JB*l,,TNH
                       EtL-?2*'4JX            qBBEZBBp5p&L&I11gBBBga/          OLE_?3*'J4h
                        \N,^`:1B}             NBE  UBBBJBBBBBBBBBBBh            1N1'_:1B}
                }yyyyyvc  IMMMS              /#M  QBBBBB,''MBBp                  )Z9M$S  mfffff>
                E!!',:|J J4BBB9\                 @BBBTNBB,|:7BBp                 7MBMBE  |:F"p.:
                L-L]:2!|%Z'?'I_|b                *9B|!'M##-|:7#                 1i.|!I_7Y':LLi[:V
                L!_`'!!l`'T; },!Tv                 m[||||||||?z                }}|!\z,'!.|:>>>'_
                     q,,,     ;<||>b1            ,z|||!||||||||L             ,Ef!|(D   t,,,1
                               O3!!|T7br,     ~shf!I|:||!||!||||>ty\      ~&vf|:]$
                                 }5j!!|T""*f""f|!.{||![[`|[r||),!!|T"*MY""||!:lC~
                gp                 1Ir1,:'!''!!'}f|!'J!|_||]`!|Tc`'''!!'!|l,]I}               sr
               ,33\        ,w>AM>+Cbr1Li]^\]*1)"|||'.t|!``|[t`!||>c1/*,C\1f>    ,,,          )bL
             )gB##BBL     2f-l;[||;!|TfMHMMfT||!|'_.P]|.].!|Tc`!!!|TMNyc,,uut7f^||T*Kc     gBB##Bp
            }3L|u1.7Es   v!|[U1gqI;~),|!!!!!!!'!`\2/||']2},||Tbs','!||||||||!||lUlsl!?\   ZB!]L}`NO
            93LT}t_]LN  )r:Jz       *pzt[u[1[I1 55f|!:}} \?.!||>51,[.,!l!l}\v11A  F1!|m   fQ`]7,_3?
             SNL11gB5   /|l5                ,wF{j|!!,?}    [f,!||>QC t+py<A        3*||    4bs11gB
              hg1}/f****:,I              }E{"|!!:.1?1       \2]1.|!T*h              z';Y***MUzwcap
              }PBfF!',k,|[7            }z*|:=;I@zID            \15!,;|Yh             ]!\gL[:?FMEM]
              t:Mkf:dB1S<|J           ?/||\Ccf                    f2l;!TH           \!!N;]#`=lgt|2
                <,l!.?P:!_  ?M        ?|[Cn     Ht              7M  */}|7z      ?M}  /!TM"f!\,,1
                   =+<+++ }Bdppp     Ct|;J    }BggZp          qpggpL 32]:0    }Bgpgp  ++++++
                         ?BM"T"MBh   ]!|*    @BM""YME        q#"""MND Kr![   TBM"""MpL
                        ?EL_{zF_43   ?!|2   gOt_P}t`Bb      d3'-KP<TLZy{!]  TFL`7zt_4qt
                         lb,"'_\B,   2!|K   ?)L_"?_\E|      l3[~*'`3t~),!3   ]L.T!_ld,
                          5MM#MY     i:;Z     ZM###M          QM##NS  1*|T    QM###M@[.....
                    ?T^^^'''$d66w   V!!1     I~~~~??/        FTT"TTT\ ?F!|t   hBpppPz|.}<,!J
                    }[lxE;],!$T*'T7f!|mz   \Y!!~,!||[7c    ]f!!lWb[:'T 1r!TT%"'")Q/.f!\>yq!7
                    .[7QBBf.T!,ls\w%s      [[r!~f~<!?!J    E[r!JBBL|?!z YJ\,l,u}c,.,'|J,t3`'/
                     ,1,,,,,}               ,'!.....'\      c|'.!:!',}              !,11,,12

                                                tentaflake.dev


  </pre>
  <p align="center">
    <i>Declaratively deploy & manage multiple isolated Hermes AI agents
    on a single NixOS machine — each with its own secrets, skills, and personality.</i>
    <br/>
    <sub>Clone → configure → rebuild. Your swarm, your NixOS, your rules.</sub>
  </p>
</p>

---

## What Is Tentaflake?

**Tentaflake** is a **NixOS** (Linux distro configured entirely in code) template for running multiple isolated **Hermes AI agents** on one machine. Each agent lives in its own Docker container with its own secrets, skills, and personality. Define all your agents in one file — the template handles servers, secrets, networking, and shells.

No SaaS, no third-party agent router — you host, you control. Clone → configure → rebuild.

---

## Quick Comparison — Choose Your Path

| You want to… | Start here | What you get |
|---|---|---|
| **Try it with zero commitment** | [⚡ Path 1: Live USB](#⚡-path-1-try-it-now--live-usb) | Boot from USB, agents run in RAM, nothing touches disk |
| **Install NixOS permanently** | [💾 Path 2: Installer ISO](#💾-path-2-install-permanently--installer-iso) | Boot from USB, TUI wizard installs NixOS + agents to disk |
| **Already use NixOS, want agents** | [🛠️ Path 3: Customize Agents](#🛠️-path-3-customize-your-agents) or [🔧 Flake Input](#🔧-for-nixos-experts-consume-as-flake-input) | Add `tentaflake` module to your existing config |

---

## ⚡ Path 1: Try It Now — Live USB

Boot any x86_64 machine from a USB stick. Agents run entirely in **RAM** — pull the USB and every trace is gone. Requires no NixOS install, no existing Nix setup.

> **You need some way to build the ISO.** The build machine is separate from the target — any Linux, macOS, or Windows box can do it. You have three options:

<details>
<summary><b>Option A: Install Nix on any Linux/macOS (recommended, 5 minutes)</b></summary>

Nix is a **package manager** — NOT NixOS the operating system. It runs on Ubuntu, Fedora, Debian, macOS, Arch, and most other Linux distros, sitting happily alongside `apt`/`dnf`/`brew`.

```bash
# Install Nix on any Linux or macOS — works alongside your existing tools
curl --proto '=https' --tlsv1.2 -sSf https://nix.dev/install-nix | sh

# Restart your shell or source the profile
. "$HOME/.nix-profile/etc/profile.d/nix.sh"
```

Then follow the build steps below.
</details>

<details>
<summary><b>Option B: Use Docker to build (no Nix install needed)</b></summary>

Don't want to install Nix? Run it in a container instead. Works on any machine with Docker (Linux, macOS, Windows):

```bash
# Clone the repo
git clone https://github.com/timfewi/tentaflake
cd tentaflake

# Use the official Nix Docker image to build the ISO
docker run --rm -v "$PWD:/build" -w /build nixos/nix \
  sh -c "nix build .#live-agent-iso --extra-experimental-features 'nix-command flakes'"

# ISO appears at result/iso/tentaflake-live.iso
```
</details>

<details>
<summary><b>Option C: Download a pre-built ISO (if available)</b></summary>

Check the [GitHub Releases](https://github.com/timfewi/tentaflake/releases) page — pre-built ISOs may be available for download. No build step needed, just download and write to USB.
</details>

### What You Need

- A machine with **8 GB+ RAM** (the target machine — Docker image ~2 GB lives in RAM)
- A USB stick (8 GB+)
- A build machine (any Linux, macOS, or Windows box — see options above)

### Build the ISO

If going with Option A (install Nix) or Option B (Docker), clone and build:

```bash
# Get the code
git clone https://github.com/timfewi/tentaflake
cd tentaflake

# Build the live ISO (takes a few mins first time)
nix build .#live-agent-iso
#     ^^^^   ^^^^^^^^^^^^^^^^
#     |      output named "live-agent-iso" from this flake
#     "nix build" builds an output (package, ISO, config)
#     .# means "from the flake in the current directory"
```

When done, the ISO is at `result/iso/tentaflake-live.iso`.
(`nix build` creates a `result` symlink pointing to the build output.)

### Write to USB

> ⚠️ **Destructive.** Triple-check `of=` is your USB device, not your disk.

```bash
lsblk                           # identify your USB, e.g. /dev/sdX
sudo dd if=result/iso/tentaflake-live.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

The ISO is a **UEFI + legacy-BIOS hybrid** — boots on modern and older machines.

### Boot and Set Up

1. Boot target machine from the USB (boot menu: usually F10/F12/Esc).
2. A text-mode login screen appears (TTY1) — the **firstboot wizard** starts automatically.
3. Enter at minimum an **OpenRouter API key**. Optional: Telegram bot token, Firecrawl key, Groq key.
4. The wizard writes keys to RAM and starts the agent containers. **Piper TTS** (text-to-speech, for voice interactions) is already serving at `http://localhost:5001/v1`.
5. Start chatting: `docker exec -it hermes-default hermes chat`

### Skip the Wizard (Unattended Boot)

Put `.env` files on a **second** USB labeled `HERMES_ENV`:

```bash
sudo mkfs.ext4 -L HERMES_ENV /dev/sdY1     # label is what matters
# copy default.env, research.env onto it
```

On boot the system auto-detects the label, copies env files in, starts agents **without prompting**.

### Persist Data Across Reboots

By design nothing survives a reboot. To keep agent memory and learned skills, attach a USB labeled `HERMES_DATA`:

```bash
sudo mkfs.ext4 -L HERMES_DATA /dev/sdZ1
```

At boot, each agent's state dir redirects onto that USB. Without it, the system stays fully ephemeral.

### RAM Requirements

| Machine RAM | Experience |
|---|---|
| **< 4 GB** | Not enough — Docker image pull fills the overlay |
| **4–8 GB** | Tight; one or two agents |
| **8 GB+** | Comfortable for default agents |

---

## 💾 Path 2: Install Permanently — Installer ISO

Build a bootable USB that installs NixOS + Tentaflake to disk via an interactive TUI wizard.

### Prerequisites

- A machine to build the ISO (see [build options](#-path-1-try-it-now--live-usb) in Path 1 — install Nix, use Docker, or download a pre-built release)
- A USB stick (8 GB+)
- A target machine (x86_64) with a blank disk or one you're willing to wipe

### Build the ISO

```bash
git clone https://github.com/timfewi/tentaflake
cd tentaflake
nix build .#installer-iso
# ISO at result/iso/tentaflake.iso
```

Or use the convenience script: `./scripts/build-iso.sh installer`

> 💡 **No Nix installed?** See Path 1 for [Docker build](#-path-1-try-it-now--live-usb) or [pre-built ISO](#-path-1-try-it-now--live-usb) options — same methods work for the installer ISO.

### Write to USB

```bash
sudo dd if=result/iso/tentaflake.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

### Boot and Install

1. Boot from USB on target machine.
2. The **TUI installer** launches automatically on TTY1.
3. Walk through the wizard (dialog-based):
   - Set hostname, username, password
   - Select target disk (**ALL DATA WILL BE WIPED**)
   - Set timezone
   - Confirm — then installer partitions (1 GB EFI + ext4 root), generates hardware config (auto-detects disks, GPU, network), runs `nixos-install` (10–15 min — NixOS compiles your system from config, downloading and building all packages)
4. After completion, system reboots into your new NixOS machine with Hermes ready.

### After Install

SSH in over Tailscale (`ssh admin@<hostname>`) and you land in a ready-to-operate
shell: a login banner shows host + agent health, and the `hermes` command drives the
agent containers (`hermes status`, `hermes logs <name>`, `hermes restart <name>`,
`hermes shell <name>`). See [`docs/06-shell.md`](docs/06-shell.md).

Then follow the [quickstart guide](docs/01-quickstart.md) to set up agent providers and start chatting.

---

## 🛠️ Path 3: Customize Your Agents

If you already have NixOS running (or just finished Path 2), define your agents with a **my-agents.nix** file and rebuild.

### Agent Definition File

Create `my-agents.nix` in the repo root. Here's a quick Nix syntax primer (it's simpler than it looks):

```
# Nix crash course (enough to edit this file):
#   { key }: expr       = function that takes an object with key "key"
#   { a = 1; b = 2; }  = object ("attrset"), semicolons NOT commas
#   [ x y z ]          = list (space-separated)
#   mkF ({...})        = function call
```

```nix
# my-agents.nix — each item in this list becomes one isolated agent container
{ mkHermesAgent }:   # mkHermesAgent is a helper that creates agent modules

[
  (mkHermesAgent {
    name    = "coding";
    envFile = "/run/secrets/hermes-coding.env";
    settings = {
      model.default = "openrouter/anthropic/claude-sonnet-4";
      model.provider = "openrouter";
      terminal.backend = "docker";
      toolsets = [ "terminal" "memory" "file" "skills" ];
    };
  })

  (mkHermesAgent {
    name    = "research";
    envFile = "/run/secrets/hermes-research.env";
    settings = {
      model.default = "openrouter/deepseek/deepseek-v4-flash";
      web.backend = "firecrawl";
      toolsets = [ "terminal" "web" "memory" "file" "skills" ];
    };
  })
]
```

Each agent gets:
- System user `hermes-<name>`
- State dir `/var/lib/hermes-<name>` (0700, owned by agent user)
- Docker container `hermes-<name>` (host networking, auto-start)
- `HERMES_HOME` pointing to its state dir

### Common `mkHermesAgent` Options

| Option | Type | Default | Description |
|---|---|---|---|
| `name` | `string` | *(required)* | Agent identifier |
| `envFile` | `path` | `null` | Path to `.env` file with API keys |
| `agenixFile` | `path` | `null` | Path to agenix-decrypted env file |
| `image` | `string` | `nousresearch/hermes-agent:latest` | OCI container image |
| `seedDir` | `path` | `null` | Dir with SOUL.md, AGENTS.md, skills/ (skills are reusable capabilities — like plugins — that extend what an agent can do, e.g. web search, file operations) |
| `settings` | `attrset` | `null` | Hermes config.yaml (model routing, toolsets, etc.) |
| `autoStart` | `bool` | `true` | Auto-start with systemd |
| `networkMode` | `string` | `"host"` | `"host"` or `"bridge"` |
| `extraVolumes` | `list` | `[]` | Extra `host:container:mode` mounts |

Full option reference: [`.agents/skills/tentaflake-repo-guidance/SKILL.md`](.agents/skills/tentaflake-repo-guidance/SKILL.md)

### Secrets: Two Patterns

**1. `.env` file (simpler, local-only)**

```nix
mkHermesAgent {
  name    = "my-agent";
  envFile = "/run/secrets/my-agent.env";
}
```

```bash
sudo mkdir -p /run/secrets
sudo cp hermes.env.example /run/secrets/my-agent.env
sudo chmod 600 /run/secrets/my-agent.env
sudo vi /run/secrets/my-agent.env   # add OPENROUTER_API_KEY=sk-or-...
```

Never commit `.env` files to Git.

**2. Agenix (encrypted in Git)**

**Agenix** is a tool that encrypts secrets as `.age` files — safe to commit, decrypted only at NixOS activation time (runtime, not evaluation).

```nix
mkHermesAgent {
  name       = "my-agent";
  agenixFile = "/run/agenix/my-agent-env";
}
```

Full guide: [`docs/04-agenix-secrets.md`](docs/04-agenix-secrets.md)

### Rebuild to Apply

```bash
git add my-agents.nix          # flakes only evaluate Git-tracked files
nix flake check                # validates syntax + evaluation (like tsc --noEmit)
sudo nixos-rebuild switch --flake .#agent-host
#     ^^^^^^^^^^^^^^^^       ^^^^^^^^^^^^^^^^^
#     build + apply config   use host config named "agent-host" from flake
#     "switch" activates it  (defined in flake.nix, change via tentaflake.hostName)
```

New agents appear as Docker containers. Remove an agent from the list, rebuild — container is pruned.

---

## 🏗️ Architecture

```
                      Agent Orchestration
                   One NixOS brain · Many tentacles

                           ,---------.
                         ,'  NixOS    `.
                        (    Flake      )
                 ┌───────`. (Config)  ,' ──────┐──────────────┐
                 │         `---------'         │              │
                 │              │              │              │
           /=====▼====\   /=====▼====\   /=====▼====\   /=====▼====\
           │ Tentacle │   │ Tentacle │   │ Tentacle │   │ Tentacle │
           │ Agent A  │   │ Agent B  │   │ Agent C  │   │ Agent N  │
           │ (coding) │   │(research)│   │(personal)│   │   (...)  │
           │          │   │          │   │          │   │          │
           │📦 Docker │   │📦 Docker │   │📦 Docker │   │📦 Docker │
           │User:     │   │User:     │   │User:     │   │User:     │
           │hermes-A  │   │hermes-B  │   │hermes-C  │   │hermes-N  │
           │State:    │   │State:    │   │State:    │   │State:    │
           │/var/lib/ │   │/var/lib/ │   │/var/lib/ │   │/var/lib/ │
           │hermes-A  │   │hermes-B  │   │hermes-C  │   │hermes-N  │
           │          │   │          │   │          │   │          │
           │🔑 Key: A │   │🔑 Key: B │   │🔑 Key: C │   │🔑 Key: N │
           │📚 Skills │   │📚 Skills │   │📚 Skills │   │📚 Skills │
           │          │   │          │   │          │   │          │
           \==========/   \==========/   \==========/   \==========/

       ───────────────── Shared Services ─────────────────
      🎤 Piper TTS   🔗 Tailscale   🗄️ Docker   🔐 Agenix
      (port 5001)    (mesh VPN)     (runtime)    (secrets)
```

### Key Design Decisions

| Decision | Rationale |
|---|---|
| **One container per agent** | Full isolation — no shared context, separate filesystems |
| **Host networking** | Containers use the host's network stack directly — agents reach Piper, Tailscale, etc. on `localhost` without port mapping |
| **SeedDir over :ro volumes** | Hermes can write learned skills; base files seed once, never overwrite |
| **Agenix or envFile** | Choose between encrypted-in-repo or plain-file secrets |
| **Template stays generic** | This repo is a template. Fork it, add your agents, keep your secrets. |

### Available Modules

| Module | What it configures |
|---|---|
| `boot.nix` | systemd-boot, EFI, kernel params |
| `hardening.nix` | Sysctl hardening, AppArmor, journald limits |
| `locale.nix` | Timezone, locale, console keymap |
| `networking.nix` | Hostname, nftables firewall, NetworkManager |
| `nix-settings.nix` | Flakes, auto-GC, trusted-users, substituters |
| `packages.nix` | curl, git, jq, tmux, vim, and more |
| `users.nix` | Admin user (wheel + networkmanager groups) |
| `shell.nix` | SSH/console operator experience — `hermes` CLI, login banner, prompt, zsh/oh-my-zsh, zoxide, lazygit, modern CLI tools ([docs](docs/06-shell.md)) |
| `editor.nix` | Optional Neovim via nvf (LSP, treesitter, telescope) — `tentaflake.editor.nvf.enable`, exported as `nixosModules.editor` ([docs](docs/06-shell.md#zsh-zoxide-lazygit-neovim)) |
| `tailscale.nix` | Tailscale with SSH + tag:auto (optional) |
| `piper-tts-server.nix` | Local TTS via Piper (OpenAI-compatible API) |
| `hermes-auditd.nix` | Filesystem audit daemon + `hermes top` live activity dashboard — [docs](docs/06-shell.md#hermes-top--live-activity-dashboard) |

### Available ISOs

| ISO | Build | Purpose |
|---|---|---|
| **Live Agent ISO** | `nix build .#live-agent-iso` | Run agents + TTS in RAM, no disk write ([Path 1](#⚡-path-1-try-it-now--live-usb)) |
| **Installer ISO** | `nix build .#installer-iso` | Bootable TUI wizard, installs to disk ([Path 2](#💾-path-2-install-permanently--installer-iso)) |

### Common Commands

```bash
nix flake check                          # validate everything builds
nix build .#installer-iso                # build installer ISO
nix build .#live-agent-iso               # build live ISO
nix build .#hermes-auditd                # build audit daemon package
sudo nixos-rebuild switch --flake .#agent-host  # deploy config
sudo nixos-rebuild dry-activate --flake .#agent-host  # dry-run
sudo nixos-rebuild switch --rollback     # undo last deploy
docker ps --filter "name=hermes-"        # list running agents
docker logs hermes-coding                # view agent logs
docker exec -it hermes-coding hermes chat  # chat with an agent
```

---

## 📚 Learning Nix

New to NixOS? These resources will get you up to speed:

| Resource | What it covers |
|---|---|
| [Zero to Nix](https://zero-to-nix.dev) | The fastest intro — Nix language, flakes, dev shells |
| [nix.dev](https://nix.dev) | Official Nix tutorials and guides |
| [NixOS Manual](https://nixos.org/manual/nixos/stable/) | Official NixOS reference |
| [Nix Pills](https://nixos.org/guides/nix-pills/) | Deep-dive into Nix internals |
| [NixOS Flakes Book](https://nixos-and-flakes.thiscute.world) | Practical flake guide |

Key concepts used in this project:

- **Flake** — a Git-tracked Nix project with a `flake.nix` entry point and `flake.lock` lockfile that pins every dependency version. `nix build .#foo` builds output `foo`.
- **`nixos-rebuild switch --flake .#host`** — builds and activates the NixOS configuration named `host` from the flake in the current directory.
- **Derivation** — a build recipe (any `.drv` file). `nix build` turns derivations into build results (packages, ISOs, etc.).
- **`nix flake check`** — validates the flake: syntax, evaluation, and builds all checks.

---

## 🔧 For NixOS Experts: Consume as Flake Input

Add tentaflake as a dependency to your own flake — useful when you already have a NixOS config and just want the agent modules:

```nix
# your-flake.nix
{
  inputs.tentaflake = {
    url = "github:timfewi/tentaflake";
    inputs.nixpkgs.follows = "nixpkgs";  # align nixpkgs version
  };

  outputs = { self, nixpkgs, tentaflake, ... }:
  let
    system = "x86_64-linux";
    mkHermesAgent = tentaflake.lib.${system}.mkHermesAgent;
  in {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      inherit system;
      specialArgs = { inherit mkHermesAgent; };
      modules = [
        tentaflake.nixosModules.default    # all base modules
        {
          tentaflake.hostName = "my-machine";
          tentaflake.adminUser = "alice";
          tentaflake.timeZone = "Europe/Vienna";
        }
        ./hardware-configuration.nix
      ] ++ import ./my-agents.nix { inherit mkHermesAgent; };
    };
  };
}
```

You get:
- **`tentaflake.nixosModules.default`** — all NixOS modules with `tentaflake.*` options
- **`tentaflake.lib.x86_64-linux.mkHermesAgent`** — agent builder helper
- **`tentaflake.lib.x86_64-linux.constants`** — default values (hostname, stateVersion, locale)

See [`examples/consumer-flake.nix`](examples/consumer-flake.nix) for a full worked example including agenix and home-manager.

---

## 🤝 Contributing

This is a **generic template** — keep it that way. No domain-specific code, real hostnames, API keys, or company config.

1. Fork the repo
2. Create a feature branch (`git checkout -b feat/amazing`)
3. Commit using [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`)
4. Run `nix flake check` and `go test ./...` in `pkgs/hermes-auditd/`
5. Open a Pull Request

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for full details.

---

## 📄 License

MIT — see [LICENSE](LICENSE).
Piper voice models distributed under their respective MIT licenses.
