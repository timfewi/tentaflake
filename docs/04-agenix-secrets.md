# Agenix Secrets — Encrypted Agent Credentials

This guide covers encrypting Hermes agent API keys and tokens with [agenix](https://github.com/ryantm/agenix) so they can be committed to Git safely and decrypted only at NixOS activation time.

## Why Agenix?

| Approach | Secrets in Git? | Secrets in Nix Store? | Complexity |
|----------|:---:|:---:|:---:|
| Plain `.env` files | ❌ (must gitignore) | ❌ (if mounted correctly) | Low |
| **Agenix** | ✅ (encrypted `.age` files) | ❌ (decrypted at runtime) | Medium |
| `builtins.readFile` | ❌ | ❌ **SECRETS IN STORE** | Low (dangerous) |
| External vault (Vault, Doppler) | N/A | N/A | High |

Agenix gives you the best balance: secrets encrypted in Git, decrypted only at activation, never in the Nix store, and no external vault dependency.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Git Repo (public/private)                                  │
│  ┌──────────────────────────────────────────────────┐      │
│  │ secrets/hermes-coding.env.age   (encrypted)       │      │
│  │ secrets/hermes-research.env.age (encrypted)       │      │
│  │ secrets.nix                     (recipients)      │      │
│  └──────────────────────────────────────────────────┘      │
└─────────────────────────────────────────────────────────────┘
                         │
                         │ nixos-rebuild switch
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  Nix Store (world-readable)                                 │
│  ┌──────────────────────────────────────────────────┐      │
│  │ /nix/store/...-hermes-coding.env.age   (copied)  │      │
│  │ /nix/store/...-hermes-research.env.age (copied)  │      │
│  │                    ↑ STILL ENCRYPTED ↑           │      │
│  └──────────────────────────────────────────────────┘      │
└─────────────────────────────────────────────────────────────┘
                         │
                         │ agenix activation script
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  Runtime (root-only, tmpfs)                                 │
│  ┌──────────────────────────────────────────────────┐      │
│  │ /run/agenix/hermes-coding-env   (plaintext, 0600) │      │
│  │ /run/agenix/hermes-research-env (plaintext, 0600) │      │
│  │                    ↑ NEVER IN STORE ↑             │      │
│  └──────────────────────────────────────────────────┘      │
└─────────────────────────────────────────────────────────────┘
                         │
                         │ Docker --env-file mount
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  Docker Container (isolated)                                │
│  │ OPENROUTER_API_KEY=sk-or-...                             │
│  │ TELEGRAM_BOT_TOKEN=...                                   │
└─────────────────────────────────────────────────────────────┘
```

## Step-by-Step Setup

### 1. Enable Agenix in `flake.nix`

Uncomment the agenix input and module import:

```nix
# flake.nix
{
  inputs = {
    # ...
    agenix = {
      url = "github:ryantm/agenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, agenix, ... }@inputs:
    # ...
    nixosConfigurations.agent-host = nixpkgs.lib.nixosSystem {
      modules = [
        inputs.agenix.nixosModules.age  # <-- add this
        ./configuration.nix
      ];
    };
}
```

### 2. Create `secrets.nix` (recipients + secret declarations)

Copy `secrets.nix.example` to `secrets.nix` and edit:

```bash
cp secrets.nix.example secrets.nix
```

Edit `secrets.nix` to set your agent names and SSH public keys:

```nix
# secrets.nix
{ config, lib, pkgs, ... }:

let
  # Your agent names (match mkHermesAgent calls)
  agentNames = [
    "hermes-coding"
    "hermes-research"
  ];

  # SSH public keys of machines that can decrypt.
  # NEVER put private keys here.
  recipients = [
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA..."  # Your dev machine
    # Host key auto-detected on NixOS — add others as needed
  ];

  mkSecret = name: {
    "${name}-env" = {
      file = ./secrets/${name}.env.age;
      owner = name;
      group = name;
      mode = "0600";
    };
  };
in
{
  age.secrets = lib.mergeAttrsList (map mkSecret agentNames);
}
```

Import it in `configuration.nix`:

```nix
# configuration.nix
{
  imports = [
    ./modules/...
    ./secrets.nix  # <-- add this
  ];
}
```

### 3. Install Agenix CLI

```bash
# One-time:
nix profile install nixpkgs#agenix

# Or use ad-hoc:
nix shell nixpkgs#agenix -c agenix --help
```

### 4. Create Encrypted `.age` Files

```bash
# Create the secrets directory
mkdir -p secrets

# Create encrypted secret files
echo "OPENROUTER_API_KEY=sk-or-..." | agenix -e secrets/hermes-coding.env.age --stdin
echo "OPENROUTER_API_KEY=sk-or-..." | agenix -e secrets/hermes-research.env.age --stdin

# Or edit interactively:
agenix -e secrets/hermes-coding.env.age
```

Each `.age` file contains the environment variables for one agent:

```
OPENROUTER_API_KEY=sk-or-v1-abc123...
TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234...
```

### 5. Wire Agenix Secrets to Agents

In `my-agents.nix`, use `agenixFile` instead of `envFile`:

```nix
# my-agents.nix
{ mkHermesAgent }:
[
  (mkHermesAgent {
    name       = "coding";
    agenixFile = "/run/agenix/hermes-coding-env";  # ← matches secrets.nix path
    settings = {
      model.default = "openrouter/anthropic/claude-sonnet-4";
      toolsets = [ "terminal" "memory" "file" "skills" ];
    };
  })
  (mkHermesAgent {
    name       = "research";
    agenixFile = "/run/agenix/hermes-research-env";
    settings = {
      model.default = "openrouter/deepseek/deepseek-v4-flash";
      toolsets = [ "terminal" "web" "memory" "file" "skills" ];
    };
  })
]
```

The runtime path follows this convention:

```
/run/agenix/<name-from-secrets.nix>
                      │
                      └─ mkSecret creates "${name}-env"
                         → path = /run/agenix/hermes-coding-env
```

### 6. Set SSH Identity for the Target Host

On the target NixOS machine, ensure the host's SSH key is available:

```nix
# In your flake.nix params or configuration.nix:
age.identityPaths = [
  "/etc/ssh/ssh_host_ed25519_key"
];
```

This is typically auto-detected on NixOS. If you use impermanence, point to the persistent path:

```nix
age.identityPaths = [
  "/persist/etc/ssh/ssh_host_ed25519_key"
];
```

### 7. Rebuild

```bash
sudo nixos-rebuild switch --flake /etc/nixos#agent-host
```

Agenix decrypts the `.age` files during activation and places plaintext at `/run/agenix/`. The Docker container mounts this as `--env-file`.

## Verification

After rebuild, verify secrets are wired without revealing contents:

```bash
# Check file exists with correct permissions
ls -l /run/agenix/
# Expected: -rw------- 1 hermes-coding hermes-coding ... hermes-coding-env

stat -c '%U %G %a %n' /run/agenix/hermes-coding-env
# Expected: hermes-coding hermes-coding 600 .../hermes-coding-env

# Check the agent container has the env file mounted
docker inspect hermes-coding | jq '.[0].Mounts[] | select(.Source | startswith("/run/agenix"))'
```

**Never** run `cat /run/agenix/*` or `agenix -d` to verify contents — use permissions and process checks instead.

## Working with Multiple Contributors

If multiple people need to edit secrets, add their SSH public keys to `secrets.nix` recipients and rekey:

```bash
# After adding new recipients to secrets.nix:
cd secrets
agenix --rekey
```

The `.age` format supports multiple recipients — each can decrypt independently.

## Security Checklist

- [ ] `agenix` input uncommented in `flake.nix`
- [ ] `inputs.agenix.nixosModules.age` imported in host modules
- [ ] `secrets.nix` created from `secrets.nix.example`
- [ ] No `builtins.readFile config.age.secrets.*.path` anywhere in Nix
- [ ] `age.identityPaths` uses runtime strings (not Nix paths)
- [ ] `owner`/`group`/`mode` set to restrict access per agent
- [ ] `.env` files excluded via `.gitignore` (`secrets/*.env`)
- [ ] `.age` files tracked in Git
- [ ] No private keys or decrypted secrets committed to Git
- [ ] Agent containers read `agenixFile` not `envFile` when using agenix

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| `agenix: command not found` | CLI not installed | `nix shell nixpkgs#agenix` |
| `.age` file not decrypting | Recipient key missing or wrong | Check `secrets.nix` recipients match `~/.ssh/id_ed25519.pub` |
| Agent can't read env file | Wrong owner/mode | Set `owner = "agent-name"` and `mode = "0600"` in `secrets.nix` |
| `/run/agenix/` empty after rebuild | Module not imported | Verify `inputs.agenix.nixosModules.age` in host modules |
| `error: path ... is not in the Nix store` | Private key referenced as Nix path | Use string: `age.identityPaths = [ "/etc/ssh/..." ]` |

## References

- [Agenix GitHub](https://github.com/ryantm/agenix)
- [NixOS Wiki — Agenix](https://wiki.nixos.org/wiki/Agenix)
- `secrets.nix.example` — template in this repo
- `docs/01-quickstart.md` — getting started with agent deployment
