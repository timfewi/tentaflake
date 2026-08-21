# Tentaflake — Agent Management Guide

> This guide contains historical direct-network and direct-secret development
> examples. They require the explicit `dev` security profile. Installed hosts
> now default to `balanced`, where shared policy enforces gVisor, non-root
> execution, read-only root, no published services, and no real credential
> files. Networking is `none` unless an exact broker policy is declared. See
> [security profiles](10-security-profiles.md) before copying an example.

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

2. For the explicit `dev` profile only, create its direct env file. Balanced
   rejects real provider env files and uses the broker flow in
   [brokered egress](12-brokered-egress.md):

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

## Secrets rotation

In `balanced`, rotate the broker-owned provider credential through the
deployment's runtime secret mechanism, then restart only the exact LLM broker.
The controller retains its revocable virtual key and never receives the real
provider value. Verify `/healthz` and audit readiness; do not print the key or
inspect it through the agent environment. Direct `/run/secrets/*.env` rotation
is a compatibility procedure for explicitly selected `dev` systems only.

---

## Multiple Agents

Each balanced agent receives a separate, policy-bounded capsule. This reduces
cross-agent authority; it is not a claim of complete isolation from kernel,
runtime, or image vulnerabilities.

| Aspect | Isolation |
|--------|-----------|
| **Container** | Separate Docker container |
| **System user** | `hermes-<name>` with own UID/GID |
| **State dir** | `/var/lib/hermes-<name>` (0700) |
| **Credentials** | Per-agent virtual broker key; real provider key stays host-side |
| **Configuration** | Separate HERMES_HOME |
| **Network** | `network=none` or one dedicated internal broker network |

Run agents of any type: coding, research, personal, automation, monitoring.
The generated mount policy and private host permissions deny direct access to
another agent's state. `dev` may deliberately weaken these properties and must
not be treated as an untrusted 24/7 boundary.

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

### Local observability

The optional observability profile sends the systemd journal to local Loki via
Alloy and provisions Grafana with Loki and Prometheus data sources. It replaces
the removed custom audit database and web console without enlarging the core.
See [observability and detection](09-observability.md).

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

The default agent images are pinned directly in their builders by OCI manifest
digest. The image digests are independent of `flake.lock`, so update them
deliberately:

```bash
# Print the current upstream digest for every tracked image
./scripts/update-agent-images.sh

# Review the output, edit lib/constants.nix by hand, then rebuild
sudo nixos-rebuild switch --flake /etc/nixos#<hostname>
```

The bump stays manual on purpose: a script that rewrites the pin for you is a
mutable tag with extra steps.

If you override `image`, it must be digest-pinned too — `mkHermesAgent` and
`mkZeroClawAgent` reject an unpinned reference at eval time, so a mutable tag
fails the build instead of silently producing a different deployment.

Write the reference as `registry/repository@sha256:digest`, **not**
`repository:tag@sha256:digest`. The docker CLI tolerates carrying both a tag and
a digest, but podman and skopeo reject it outright:

```
Docker references with both a tag and digest are currently not supported
```

Since `tentaflake.containerBackend` supports podman, the tag-plus-digest form
would break those hosts; keep the version in a comment instead.

For an image you build locally, there is no registry digest to pin to. On an
explicitly selected `dev` host only, set `allowMutableImage = true;` to
acknowledge it is not reproducible. Balanced/strict reject this escape hatch:

```nix
(mkHermesAgent {
  name = "coding";
  image = "my-hermes:local";
  allowMutableImage = true;
})
```

---

## Performance Tuning

### Memory limits

For `balanced`, set the shared enforced limits in the security profile. The
container policy appends these flags after caller configuration, so
`extraContainerConfig` cannot weaken or replace them:

```nix
tentaflake.security.resources = {
  memory = "4g";
  memorySwap = "4g"; # no additional swap
  cpus = "0.5";
  nofile = 4096;
};
```

`extraContainerConfig` resource overrides are a `dev` compatibility technique,
not a way to tune a secure capsule.

### Process limits

Every agent container gets `--pids-limit=512` by default — a fork-bomb ceiling
generous enough for compile jobs. A balanced agent may raise it only to another
positive ceiling; `null` (unlimited) is rejected by the secure profile:

```nix
(mkHermesAgent {
  name      = "coding";
  pidsLimit = 1024;   # heavy parallel builds; must stay positive in balanced
})
```

### Capabilities

`balanced` always enforces `cap-drop=ALL`; it is not opt-in and cannot be
removed with `extraContainerConfig`. Build dependencies into a reviewed,
digest-pinned image or run them through the disposable worker rather than
granting Linux capabilities to a long-lived controller. The `dev` profile is
the only compatibility path for experiments requiring broader authority.

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

### Env files and credentials

`/run/secrets/` is a tmpfs mount — contents never written to disk. It is a
valid location for the **broker's** Agenix-decrypted provider credentials, but
not for credentials mounted into a `balanced` or `strict` agent. Such agents
receive only their revocable virtual broker key and use the configured broker
endpoint; the broker retains the real provider, GitHub, and fetch credentials.

Direct agent env files are a `dev`-profile compatibility path only. They are
lost at reboot unless recreated by Agenix or another secret manager. Never put
them in `/etc/nixos/`: that can put their contents in the world-readable Nix
store.

### Docker isolation

`balanced` agents run in their own internal capsule network, not with host
networking. The shared builders enforce an explicit non-root uid/gid,
`cap-drop=ALL`, `no-new-privileges`, a read-only root filesystem, gVisor
`runsc`, private tmpfs paths, and CPU/RAM/swap/PID/file limits. Only the
agent's State and Workspace mounts are writable. The broker is the only
configured external authority.

The `dev` profile intentionally retains a compatibility path with broader OCI
options. It is not suitable for an untrusted 24/7 agent and must never be used
as an implicit fallback from `balanced`.

### System user security

Each agent has its own system user `hermes-<name>` with:
- No login shell (isSystemUser)
- Home directory = state dir
- No sudo access

### Detection and evidence

Journald is the primary host and container-unit evidence source. The optional
observability profile retains it in Loki; the separate Falco profile adds
kernel runtime detection. Neither profile is container isolation.

---

## Config Tips (`settings` parameter)

When using the `settings` parameter on `mkHermesAgent`, keep these in mind.

### Provider configuration

For `balanced`, define the permitted provider/models and real credentials in
`tentaflake.broker.agents.<name>` and configure the runtime to use that local
LLM endpoint with its virtual agent key. Do not add OpenRouter, Groq,
Firecrawl, GitHub, or other real provider keys to an agent env file.

The following direct variables are legacy `dev` configuration only:

| Setting | Direct variable |
|---------|-----------------|
| `model.provider = "openrouter"` | `OPENROUTER_API_KEY` |
| `stt.provider = "groq"` | `GROQ_API_KEY` |
| `web.backend = "firecrawl"` | `FIRECRAWL_API_KEY` |

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

The default Hermes agent container (`docker.io/nousresearch/hermes-agent`) is
**Python-based** and may not include Node.js. MCP servers using `npx`
will fail with "command not found". Solutions:

1. **Build a custom Docker image** extending the Hermes one with Node.js
2. **Use a Python-based MCP server** (e.g. `mcp-server-filesystem` Python package)
3. **Build a pinned custom agent image** containing Node.js when it is
   required. Do not bind-mount `/usr/bin/node` or other host binaries into a
   secure capsule.

4. **Use the broker fetch gateway** for web retrieval in `balanced`. It
   constrains destinations, redirects, DNS answers, response size, and
   provenance. The optional Hive Research module
   (`modules/optional/hive-research.nix`) is outside the core and may be used
   by an operator or an explicit `dev` integration, but a balanced capsule
   cannot reach host loopback and must not be given its key-bearing endpoint:

   ```nix
   services.hive-research = {
     enable  = true;
     package = inputs.hive-research.packages.${pkgs.system}.default;
     keyFiles.BRAVE_API_KEY_FILE = "/run/agenix/hive-brave-api-key";
   };
   ```

   A `dev`-profile agent can explicitly configure its MCP client with:

   ```yaml
   mcp_servers:
     hive-research:
       url: "http://127.0.0.1:7815/mcp"
   ```

### Optional Piper voice files

Piper is not part of the core. A secure agent image must contain any required
voice assets at build time and be pinned by digest; mounting host voice paths
is a `dev`-only compatibility option. Online TTS also needs an explicit
brokered/policy-controlled integration rather than direct agent egress.

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
