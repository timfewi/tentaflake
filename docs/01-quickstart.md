# NixOS Agent Orchestration — Quickstart Guide

Welcome to your new headless agent machine. NixOS installed, Docker ready,
Hermes containers configured. This guide walks the first 10 minutes after boot.

---

## 5-Minute Checklist

### 1. Log in and verify Docker

```bash
sudo docker ps
```

Expect empty list or `CONTAINER ID   IMAGE   COMMAND   CREATED   STATUS   PORTS   NAMES`
if no agents configured yet.

Check Docker service health:

```bash
sudo systemctl status docker
```

### 2. Copy the agent definition

```bash
cp /etc/nixos/my-agents.nix.example /etc/nixos/my-agents.nix
```

This activates a single "coding" agent. Edit later to add more.

If `/etc/nixos/my-agents.nix` doesn't exist on your system, copy the example:

```bash
sudo cp /etc/nixos/my-agents.nix.example /etc/nixos/my-agents.nix
sudo vi /etc/nixos/my-agents.nix
```

### 3. Create env file with API key

```bash
sudo mkdir -p /run/secrets
sudo cp /etc/nixos/hermes.env.example /run/secrets/hermes-coding.env
sudo chmod 600 /run/secrets/hermes-coding.env
sudo vi /run/secrets/hermes-coding.env
```

Set at minimum:

```
OPENROUTER_API_KEY=sk-or-...
```

Depending on which features you enable in `settings`, you may also need:

| Feature | Key |
|---------|-----|
| STT (speech-to-text) | `GROQ_API_KEY=gsk_...` |
| Web search | `FIRECRAWL_API_KEY=fc-...` |
| Higher rate limits | `ANTHROPIC_API_KEY=sk-ant-...` |
| OpenAI models | `OPENAI_API_KEY=sk-proj-...` |

### 4. Rebuild the system

```bash
sudo nixos-rebuild switch --flake /etc/nixos#<hostname>
```

Replace `<hostname>` with your machine's hostname (set during install).
Find it:

```bash
hostname
```

First rebuild pulls the Hermes Docker image (~2-5 min depending on network).
Subsequent rebuilds are fast.

### 5. Verify containers running

```bash
sudo docker ps
```

Expected output:

```
CONTAINER ID   IMAGE                                        ...   NAMES
abc123def456   ghcr.io/nousresearch/hermes-agent:latest     ...   hermes-coding
```

If container not running:

```bash
sudo docker ps -a
sudo docker logs hermes-coding
```

### 6. Check agent logs

```bash
sudo docker logs hermes-coding
```

First-run startup messages:

```
Starting Hermes gateway...
No provider configured. Run: hermes setup --portal
```

This is expected — provider setup comes next.

### 7. Enter the container and configure provider

```bash
sudo docker exec -it hermes-coding hermes chat
```

Inside the container, set up an LLM provider:

**Fastest — Nous Portal (OAuth browser flow):**

```bash
hermes setup --portal
```

Opens browser for login, picks a model, ready to chat.

**Alternative — OpenRouter (API key):**

```bash
hermes config set OPENROUTER_API_KEY sk-or-...
hermes config set model.provider openrouter
hermes config set model.default anthropic/claude-sonnet-4-20250514
```

**Alternative — direct Anthropic:**

```bash
hermes config set ANTHROPIC_API_KEY sk-ant-...
hermes config set model.provider anthropic
```

After setup, type `exit` to leave the container shell.

### 8. Start chatting

```bash
sudo docker exec -it hermes-coding hermes chat
```

Test with:

```
> What time is it?
> Run uname -a
```

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| Container not starting | Missing env file | Check `/run/secrets/hermes-*.env` exists and `chmod 600` |
| `nixos-rebuild` fails | Syntax error in nix file | `nix flake check` to validate |
| Container exits immediately | No provider configured | Enter container: `sudo docker exec -it hermes-coding hermes setup --portal` |
| Docker not starting | Time sync issue | `sudo timedatectl set-ntp true && sudo systemctl restart docker` |
| DNS resolution fails inside container | Host DNS config | Check `/etc/resolv.conf` on host, `ping google.com` |
| Permission denied on env file | Wrong owner/mode | `sudo chmod 600 /run/secrets/hermes-*.env` |
| `docker exec: "hermes" not found` | Container not fully started | Wait 30s, check `sudo docker logs hermes-coding` |

### Time sync

Headless machines without RTC often drift. Fix:

```bash
sudo timedatectl set-ntp true
timedatectl status
```

### Firewall

The `networking.nix` module enables nftables. Default allows SSH (port 22)
and Hermes gateway ports. Check rules:

```bash
sudo nft list ruleset
```

---

## Next Steps

- [02-agent-tips.md](02-agent-tips.md) — Day-to-day agent management
- [03-skill-index.md](03-skill-index.md) — Bundled skills reference
- [04-agenix-secrets.md](04-agenix-secrets.md) — Encrypt agent credentials with agenix
