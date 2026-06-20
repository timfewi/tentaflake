---
name: hermes-config-manager
description: Manage Hermes Agent configuration — config.yaml, .env, profiles, terminal backends, models, tools, env substitution, migration
version: 1.0.0
platforms: [linux, macos, windows]
metadata:
  hermes:
    tags: [config, setup, profile, terminal]
    category: devops
    requires_toolsets: [terminal]
---

# Hermes Config Manager

## When to Use

- Set up or modify Hermes agent configuration
- Switch terminal backends (local/docker/ssh/modal/daytona/singularity)
- Configure model provider and API keys
- Manage profiles
- Migrate config after update
- Debug config issues
- Enable/disable toolsets globally

## Procedure

### 1. Directory Structure

```
~/.hermes/
├── config.yaml     # Settings — model, terminal, TTS, compression
├── .env            # API keys and secrets (chmod 600)
├── auth.json       # OAuth provider credentials
├── SOUL.md         # Agent identity (slot #1 in system prompt)
├── memories/       # MEMORY.md, USER.md
├── skills/         # Agent skills
├── cron/           # Scheduled jobs
├── sessions/       # Gateway sessions
└── logs/           # errors.log, gateway.log
```

### 2. Key Commands

```bash
hermes config              # View current config
hermes config edit         # Open in $EDITOR
hermes config set KEY VAL  # Set value (API keys → .env, rest → config.yaml)
hermes config check        # Find missing options after update
hermes config migrate      # Interactively add missing options
hermes config show         # Display full config
```

`hermes config set` auto-routes: API keys → `.env`, everything else → `config.yaml`.

### 3. Config Precedence

Settings resolved in order (highest first):

1. CLI args (`hermes chat --model X`)
2. `~/.hermes/config.yaml`
3. `~/.hermes/.env` (fallback for env vars)
4. Built-in hardcoded defaults

**Rule**: Secrets go in `.env`. Everything else in `config.yaml`. When both set, `config.yaml` wins for non-secrets.

### 4. Profile Management

```bash
hermes -p work chat                    # Start session with work profile
hermes profile create research         # Create new profile
hermes profile create research --no-skills  # Profile without bundled skills
hermes profile list                    # List all profiles
hermes profile switch work             # Set default profile
```

Profiles get isolated `HERMES_HOME`, config, memory, sessions, and gateway PID. Run concurrently.

### 5. Terminal Backend Configuration

```yaml
terminal:
  backend: local # local | docker | ssh | modal | daytona | singularity
  cwd: "." # Gateway working dir (CLI uses launch dir)
  timeout: 180 # Per-command timeout (seconds)
  env_passthrough: [] # Env vars to forward to sandboxes
  persistent_shell: true # Single bash session across commands (SSH default: true)
```

**Set from CLI:**

```bash
hermes config set terminal.backend docker
hermes config set terminal.docker_image nikolaik/python-nodejs:python3.11-nodejs20
```

**Docker-specific:**

```yaml
terminal:
  docker_image: "nikolaik/python-nodejs:python3.11-nodejs20"
  docker_mount_cwd_to_workspace: false
  docker_run_as_host_user: false
  docker_forward_env:
    - "GITHUB_TOKEN"
  docker_env:
    DEBUG: "1"
  docker_volumes:
    - "/home/user/projects:/workspace/projects"
  docker_extra_args:
    - "--gpus=all"
```

Env var overrides: `TERMINAL_DOCKER_IMAGE`, `TERMINAL_DOCKER_VOLUMES` (JSON array), etc.

**SSH:**

```bash
export TERMINAL_SSH_HOST=my-server.example.com
export TERMINAL_SSH_USER=ubuntu
# Optional:
export TERMINAL_SSH_PORT=22
export TERMINAL_SSH_KEY=~/.ssh/id_ed25519
```

**Modal:**

```yaml
terminal:
  backend: modal
  container_cpu: 1
  container_memory: 5120
  container_disk: 51200
  container_persistent: true
```

Requires `MODAL_TOKEN_ID` + `MODAL_TOKEN_SECRET` or `~/.modal.toml`.

### 6. Model/Provider Configuration

```yaml
model:
  provider: nous # nous | openrouter | anthropic | openai | custom
  default: anthropic/claude-sonnet-4.6
  base_url: https://inference-api.nousresearch.com/v1
```

Switch at CLI:

```bash
hermes config set model anthropic/claude-sonnet-4.6
hermes config set model.provider openrouter
```

Multi-provider fallback:

```yaml
model:
  provider: openrouter
  fallbacks:
    - provider: nous
    - provider: anthropic
```

### 7. Tool Configuration Per Platform

```bash
hermes tools    # Interactive tool config per platform
```

Write tool config to specific platforms (CLI, telegram, discord, etc.). Creates platform-specific toolset presets.

Global toolset disable in config:

```yaml
agent:
  disabled_toolsets:
    - memory
    - web
```

### 8. Skill Settings in Config

```yaml
skills:
  config:
    myplugin:
      path: ~/myplugin-data
  guard_agent_created: false # Set true to scan skill writes for dangerous patterns
  external_dirs:
    - ~/.agents/skills
```

### 9. Env Var Substitution in config.yaml

Use `${VAR_NAME}` syntax:

```yaml
auxiliary:
  vision:
    api_key: ${GOOGLE_API_KEY}
    base_url: ${CUSTOM_VISION_URL}
```

Multiple refs in one value work: `url: "${HOST}:${PORT}"`. Unset vars keep placeholder verbatim. Only `${VAR}` supported — bare `$VAR` not expanded.

### 10. Config Migration

```bash
hermes config check       # Find missing options after update
hermes config migrate     # Walk through new options interactively
```

After Hermes update always run `hermes config check` to find new required or optional config keys.

### 11. Provider Timeouts

```yaml
providers:
  openrouter:
    request_timeout_seconds: 1800
    stale_timeout_seconds: 300
    models:
      claude-sonnet-4:
        timeout_seconds: 3600
```

### 12. Compression Settings

```yaml
compression:
  enabled: true
  threshold: 0.50 # Compress at 50% of context limit
  target_ratio: 0.20
  protect_last_n: 20
  protect_first_n: 3
  hygiene_hard_message_limit: 400

auxiliary:
  compression:
    model: "" # Empty = use main chat model
    provider: auto
    base_url: null
```

### 13. Memory Config

```yaml
memory:
  memory_enabled: true
  user_profile_enabled: true
  memory_char_limit: 2200
  user_char_limit: 1375
  write_approval: false # false=write freely, true=require approval
```

### 14. File Read & Tool Output Limits

```yaml
file_read_max_chars: 100000 # Max chars per read_file call

tool_output:
  max_bytes: 50000 # Terminal output truncation
  max_lines: 2000 # read_file pagination cap
  max_line_length: 2000 # Per-line cap
```

### 15. Credential Pool Strategies

```yaml
credential_pool_strategies:
  openrouter: round_robin # fill_first | round_robin | least_used | random
  anthropic: least_used
```

## Pitfalls

- **YAML duplicate keys**: Silent override. Merge new mounts into same `docker_volumes:` list
- **Secrets in config.yaml**: Never put API keys in `config.yaml` — use `.env` with `chmod 600`
- **Context length wrong**: Set `model.context_length` if auto-detection fails
- **Local model timeouts**: Hermes auto-detects local endpoints and relaxes timeouts. Set `HERMES_STREAM_READ_TIMEOUT=1800` if still hitting limits
- **Switching profiles loses `--continue`**: Each profile has own session db. Use `hermes -p <name> -c`
- **`hermes config set` routes .env**: API key names (`OPENROUTER_API_KEY`) auto-detect as secrets
- **`hermes config migrate` only checks enabled skills**: Disabled skills' config settings skipped
- **Gateway hot-reload**: Editing `model.context_length` or `compression.*` takes effect on next message. API keys and tool/skill config need `/reload-mcp` or restart
- **SSH with persistent shell**: Don't enable `TERMINAL_LOCAL_PERSISTENT` unless needed for stateful commands

## Verification

```bash
hermes config show | head -30                        # View current config
hermes config check                                    # Check for missing options
hermes doctor                                          # Full diagnostics
hermes gateway status                                  # Gateway state
cat ~/.hermes/.env                                     # Verify secrets file
```
