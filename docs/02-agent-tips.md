# Tentaflake — Agent Management Guide

Day-to-day operations for your Hermes agents: state, logs, secrets,
updates, security.

---

## State Directories

Each agent gets an isolated state directory at `/var/lib/hermes-<name>`:

```
/var/lib/hermes-coding/
├── workspace/      # Agent working directory — files, clones, output
├── skills/         # Loaded skill files
├── cron/           # Scheduled task definitions
└── ...             # Hermes internal state (config, sessions, logs)
```

Inside the container, `HERMES_HOME` points here. All agent persistence lives
in this tree.

List all agent state dirs:

```bash
ls -la /var/lib/ | grep hermes
```

---

## Managing Agents

### Via systemd

Docker containers are managed by systemd. List agent services:

```bash
sudo systemctl list-units | grep hermes
```

Expected:

```
docker-hermes-coding.service    loaded active running   Docker Application Container hermes-coding
```

Start / stop / restart:

```bash
sudo systemctl start docker-hermes-coding
sudo systemctl stop docker-hermes-coding
sudo systemctl restart docker-hermes-coding
sudo systemctl status docker-hermes-coding
```

### Via Docker

```bash
# List running agent containers
sudo docker ps --filter "name=hermes-"

# List all (including stopped)
sudo docker ps -a --filter "name=hermes-"

# View logs
sudo docker logs hermes-coding
sudo docker logs --tail 50 -f hermes-coding   # tail + follow

# Restart
sudo docker restart hermes-coding

# Enter container shell
sudo docker exec -it hermes-coding bash

# Run Hermes command directly
sudo docker exec -it hermes-coding hermes chat
sudo docker exec -it hermes-coding hermes model
```

### Inspect container config

```bash
sudo docker inspect hermes-coding | jq '.[0].Config.Env'
sudo docker inspect hermes-coding | jq '.[0].Mounts'
```

---

## Adding / Removing Agents

**Add an agent:**

1. Edit `my-agents.nix` — append an attrset to the `hermesAgents` list:

   ```nix
   {
     name    = "personal";
     envFile = "/run/secrets/hermes-personal.env";
   }
   ```

   See the commented reference agent in `my-agents.nix.example` for every
   available `settings` / volume / container option.

   A second runtime, ZeroClaw, works the same way via the `zeroclawAgents`
   list (agents get container/state dir `zeroclaw-<name>`, config from a
   `settings` attrset serialized to TOML instead of YAML). See the
   commented reference entry in `my-agents.nix.example` and
   `zeroclaw.env.example` for its env-file convention.

2. Create env file:

   ```bash
   sudo cp /etc/nixos/hermes.env.example /run/secrets/hermes-personal.env
   sudo chmod 600 /run/secrets/hermes-personal.env
   sudo vi /run/secrets/hermes-personal.env
   ```

3. Rebuild:

   ```bash
   sudo nixos-rebuild switch --flake /etc/nixos#<hostname>
   ```

**Remove an agent:** Delete or comment out the agent block, rebuild.
Container, system user, and state dir remain on disk. Clean up manually:

```bash
sudo rm -rf /var/lib/hermes-<name>
sudo userdel hermes-<name>
sudo groupdel hermes-<name>
```

---

## Secrets Rotation

Env files live at `/run/secrets/hermes-<name>.env` (tmpfs — never on disk).

**Rotate a key:**

```bash
sudo vi /run/secrets/hermes-coding.env
# Edit the key, save
sudo chmod 600 /run/secrets/hermes-coding.env
sudo systemctl restart docker-hermes-coding
```

Container re-reads env file on restart. No rebuild needed for key changes.

**Verify new key loaded:**

```bash
sudo docker exec hermes-coding env | grep API_KEY
```

---

## Multiple Agents

Each agent is fully isolated:

| Aspect | Isolation |
|--------|-----------|
| **Container** | Separate Docker container |
| **System user** | `hermes-<name>` with own UID/GID |
| **State dir** | `/var/lib/hermes-<name>` (0700) |
| **API keys** | Separate env file per agent |
| **Configuration** | Separate HERMES_HOME |
| **Network** | Host networking — all agents share host net |

Run agents of any type: coding, research, personal, automation, monitoring.
They cannot read each other's state or context.

---

## Logging

### Docker logs

```bash
sudo docker logs hermes-coding
sudo docker logs --tail 100 -f hermes-coding
```

### Journald (systemd view)

```bash
sudo journalctl -u docker-hermes-coding
sudo journalctl -u docker CONTAINER_NAME=hermes-coding
```

### Agent internal logs

Inside the container, Hermes writes to:

```bash
sudo docker exec hermes-coding cat $HERMES_HOME/logs/errors.log
sudo docker exec hermes-coding cat $HERMES_HOME/logs/gateway.log
```

### Audit daemon (if enabled)

The `hermes-auditd` service records **filesystem changes** inside every
declarative agent container's state dir — `/var/lib/hermes-<name>/` and
`/var/lib/zeroclaw-<name>/` alike, auto-discovered from whatever's defined in
`my-agents.nix` — which files an agent creates, writes, removes, renames, or
chmods, with a timestamp and size. (It does *not* capture the agent's
conversation or commands — for that, use `tentaflake logs <name>`.)

The fastest way to see this is the live dashboard:

```bash
tentaflake top     # live TUI: per-agent activity + scrolling event log
```

Prefer a browser? Enable the **Agent Console** (`tentaflake.hermes-auditd.console.enable`)
for a tailnet-served web page that pairs a read-only, secrets-excluded file explorer
across all agents with the same live activity feed — no per-agent dashboard logins.
See [`docs/06-shell.md`](06-shell.md#agent-console--web-file-explorer--live-monitor).

The daemon's own service log:

```bash
sudo journalctl -u hermes-auditd
```

Enable it with `tentaflake.hermes-auditd.enable = true;` (on by default for the
`agent-host` config). See [`docs/06-shell.md`](06-shell.md#tentaflake-top--live-activity-dashboard).

---

## Backups

Save these for disaster recovery:

```
/etc/nixos/                    # Full system config (flake + modules)
/var/lib/hermes-*/workspace/   # Agent working files (selective)
/var/lib/hermes-*/cron/        # Scheduled task definitions
```

Env files are on tmpfs (`/run/secrets/`) — **lost on reboot**.
Store keys in a password manager.

**Quick backup script:**

```bash
#!/usr/bin/env bash
BACKUP="/root/backup-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP"
cp -r /etc/nixos "$BACKUP/nixos"
for d in /var/lib/hermes-*; do
  [ -d "$d/cron" ] && cp -r "$d/cron" "$BACKUP/$(basename $d)-cron"
done
tar czf "$BACKUP.tar.gz" "$BACKUP"
echo "Backup: $BACKUP.tar.gz"
```

---

## Updating Containers

Agent images are pinned in the flake. To update:

```bash
# Update flake lock (pulls latest hermes-agent image reference)
nix flake update

# Rebuild to pull new images
sudo nixos-rebuild switch --flake /etc/nixos#<hostname>
```

Docker images update automatically on rebuild when the tag changes.
For `:latest` tag, force pull:

```bash
sudo docker pull ghcr.io/nousresearch/hermes-agent:latest
sudo systemctl restart docker-hermes-coding
```

---

## Performance Tuning

### Memory limits

Set container memory limits via `extraContainerConfig` in `my-agents.nix`:

```nix
(mkHermesAgent {
  name    = "coding";
  envFile = "/run/secrets/hermes-coding.env";
  extraContainerConfig = {
    memory = "4g";
    memorySwap = "2g";
    cpuPeriod = 100000;
    cpuQuota = 50000;  # ~0.5 CPU core
  };
})
```

### Process limits

Every agent container gets `--pids-limit=512` by default — a fork-bomb ceiling
generous enough for compile jobs. Tune it (or disable with `null`) via the
`pidsLimit` parameter on `mkHermesAgent`/`mkZeroClawAgent`:

```nix
(mkHermesAgent {
  name      = "coding";
  envFile   = "/run/secrets/hermes-coding.env";
  pidsLimit = 1024;   # heavy parallel builds; null = unlimited
})
```

### Dropping capabilities (opt-in)

For agents that never need root-style operations inside the container, drop
all Linux capabilities via `extraContainerConfig`:

```nix
extraContainerConfig = {
  # Overriding extraOptions replaces the default network flag, so restate it.
  # (--env-file, --security-opt and --pids-limit are appended after the merge
  # and survive this override.)
  extraOptions = [
    "--network=host"
    "--cap-drop=ALL"
  ];
};
```

> **Warning:** agents that run `sudo`, `apt install`, or otherwise install
> packages inside the container need capabilities (`CAP_SETUID`,
> `CAP_CHOWN`, …) — `--cap-drop=ALL` will break them. That's why this is
> opt-in, not the default.

### Resource monitoring

```bash
# Per-container stats
sudo docker stats hermes-coding

# System-wide
htop
sudo journalctl -u docker-hermes-coding --since "1 hour ago" | grep -i oom
```

### Disk usage

```bash
du -sh /var/lib/hermes-*/
```

---

## Security Notes

### Env files on tmpfs

`/run/secrets/` is a tmpfs mount — contents never written to disk.
Files persist only until reboot. Recreate after each boot via:
- Manual copy from `/etc/nixos/hermes.env.example`
- Or automate via systemd tmpfiles / agenix

Do NOT store env files in `/etc/nixos/` (ends up in Nix store, world-readable).

### Docker isolation

Containers run with:
- Host networking (`--network=host`)
- Non-root inside the container — the image's built-in `hermes` user (no host `--user` override)
- Read-only state dir permissions (0700, owner only)
- No privilege escalation (`--security-opt=no-new-privileges:true`)
- A process ceiling (`--pids-limit=512`, tunable via `pidsLimit`)

### System user security

Each agent has its own system user `hermes-<name>` with:
- No login shell (isSystemUser)
- Home directory = state dir
- No sudo access

### Audit trail

If `hermes-auditd` is enabled, every filesystem change in the agents' state
dirs (across all runtimes) is recorded to a SQLite database (24h retention by
default, size capped at ~40 MB so an event flood cannot fill the disk).
Review it live with `tentaflake top`, or inspect the daemon's own log:

```bash
sudo journalctl -u hermes-auditd --since today
```

---

## Config Tips (`settings` parameter)

When using the `settings` parameter on `mkHermesAgent`, keep these in mind.

### Required API keys by feature

| Setting | API key needed |
|---------|---------------|
| `model.provider = "openrouter"` | `OPENROUTER_API_KEY` |
| `stt.provider = "groq"` | `GROQ_API_KEY` |
| `web.backend = "firecrawl"` | `FIRECRAWL_API_KEY` |
| `tts.provider = "piper"` | None (local, but needs voice files) |
| `tts.provider = "edge"` | None (built into container) |

Set these in `/run/secrets/hermes-<name>.env`.

### Model provider

Always add `model.provider` alongside `model.default`:

```nix
model = {
  default  = "deepseek/deepseek-v4-flash";
  provider = "openrouter";  # ← required, not inferred from model name
};
```

### Compression

Protect recent context and system prompt from compression:

```nix
compression = {
  enabled         = true;
  threshold       = 0.50;    # compress at 50% context fill
  target_ratio    = 0.20;    # compress to 20% of original
  protect_last_n  = 20;      # keep last 20 messages uncompressed
  protect_first_n = 3;       # keep first 3 (system prompt) uncompressed
};
```

Without `protect_last_n`/`protect_first_n`, the agent's system prompt and
recent conversation get compressed — losing identity and continuity.

### MCP Servers & Node.js

The Hermes agent container (`ghcr.io/nousresearch/hermes-agent:latest`) is
**Python-based** and may not include Node.js. MCP servers using `npx`
will fail with "command not found". Solutions:

1. **Build a custom Docker image** extending the Hermes one with Node.js
2. **Use a Python-based MCP server** (e.g. `mcp-server-filesystem` Python package)
3. **Mount Node from host**: `extraVolumes = [ "/usr/bin/node:/usr/bin/node:ro" ]`

4. **Run the MCP server on the host** — no Node.js or subprocess needed in
   the container at all. Because agents use host networking, an HTTP MCP
   server on the host is reachable at `127.0.0.1`. Tentaflake ships a
   module for [hive-research](modules/hive-research.nix), a unified
   web-research MCP server (search / extract / crawl / contacts across
   Brave, Tavily, FireCrawl, Hunter.io and Spider Cloud with failover):

   ```nix
   services.hive-research = {
     enable  = true;
     package = inputs.hive-research.packages.${pkgs.system}.default;
     keyFiles.BRAVE_API_KEY_FILE = "/run/agenix/hive-brave-api-key";
   };
   ```

   Then in each agent profile's `config.yaml`:

   ```yaml
   mcp_servers:
     hive-research:
       url: "http://127.0.0.1:7815/mcp"
   ```

### TTS Piper voice files

Piper TTS needs voice model files on disk. They are not in the container
by default. Either:
- **Mount from host**: `extraVolumes = [ "/usr/share/piper-voices:/usr/share/piper-voices:ro" ]`
- **Switch to Edge TTS**: `tts.provider = "edge"` (no files needed, online)

### Toolsets

`["all"]` enables every toolset including risky ones (Docker, package
management, system operations). Prefer explicit:

```nix
toolsets = [ "terminal" "web" "memory" "file" "skills" ];
```

### Provider timeouts

DeepSeek models can have long generation times. Set explicit timeouts:

```nix
settings = {
  providers.openrouter = {
    request_timeout_seconds = 1800;
    stale_timeout_seconds   = 300;
  };
};
```
