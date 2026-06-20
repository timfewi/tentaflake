---
name: hermes-tools-config
description: Configure Hermes tools and toolsets — enable/disable per platform, manage backends, set truncation limits, browser, web, code exec
version: 1.0.0
platforms: [linux, macos, windows]
metadata:
  hermes:
    tags: [tools, toolsets, configuration, backends, browser, web]
    category: devops
    requires_toolsets: [terminal]
---

# Hermes Tools Configuration

## When to Use

- Enable/disable specific tools per platform
- Configure tool backends (web, browser, TTS, image gen)
- Set tool output truncation limits
- Configure file read safety limits
- Disable toolsets globally
- Set up browser backends
- Configure code execution environment
- Manage terminal backend

## Procedure

### 1. Tool vs Toolset Distinction

**Tool**: Individual function (e.g., `web_search`, `terminal`, `read_file`, `memory`).
**Toolset**: Group of related tools (e.g., `web`, `terminal`, `file`, `memory`, `browser`).

Hermes ships ~70+ tools across ~28 toolsets.

### 2. hermes tools Command

Interactive per-platform configuration:

```bash
hermes tools
```

Shows each tool/toolset and lets you enable/disable per platform (CLI, Telegram, Discord, etc.).

Platform-specific toolsets:

| Platform       | Toolset                |
| -------------- | ---------------------- |
| CLI            | `hermes-cli`           |
| Telegram       | `hermes-telegram`      |
| Discord        | `hermes-discord`       |
| Slack          | `hermes-slack`         |
| WhatsApp       | `hermes-whatsapp`      |
| Email          | `hermes-email`         |
| Home Assistant | `hermes-homeassistant` |

Full tools including terminal available on most messaging platforms.

### 3. Common Toolsets

```bash
hermes chat --toolsets "web,terminal,skills"
```

| Toolset          | Tools                                              | Description                    |
| ---------------- | -------------------------------------------------- | ------------------------------ |
| `web`            | `web_search`, `web_extract`                        | Search and extract web content |
| `terminal`       | `terminal`, `process`                              | Execute commands               |
| `file`           | `read_file`, `write_file`, `patch`, `search_files` | File operations                |
| `browser`        | `browser_navigate`, `browser_snapshot`, etc.       | Browser automation             |
| `memory`         | `memory`                                           | Persistent memory              |
| `session_search` | `session_search`                                   | Search past sessions           |
| `vision`         | `vision_analyze`                                   | Image analysis                 |
| `image_gen`      | `image_generate`                                   | Image generation               |
| `tts`            | `text_to_speech`                                   | Text-to-speech                 |
| `code_execution` | `execute_code`                                     | Sandboxed code                 |
| `delegation`     | `delegate_task`                                    | Subagent spawn                 |
| `cronjob`        | `cronjob`                                          | Scheduled tasks                |
| `skills`         | `skill_manage`, `skill_view`, `skills_list`        | Skill management               |
| `messaging`      | `send_message`                                     | Outbound messaging             |
| `clarify`        | `clarify`                                          | Clarification tool             |
| `homeassistant`  | `ha_*`                                             | Home Assistant control         |
| `mcp-<server>`   | MCP tools                                          | Dynamic per-server             |

### 4. Per-Platform Tool Configuration

Tools can be enabled/disabled per platform:

```yaml
# In ~/.hermes/config.yaml (written by hermes tools)
platform_toolsets:
  telegram:
    enabled: [terminal, web, file, skills, memory]
  discord:
    enabled: [terminal, web, file, skills]
```

Or disable globally:

```yaml
agent:
  disabled_toolsets:
    - memory # Hide memory tools everywhere
    - web # No web_search/web_extract anywhere
```

This applies **after** per-platform config. Use for "turn X off everywhere."

### 5. Tool Output Truncation Limits

```yaml
tool_output:
  max_bytes: 50000 # Terminal output cap (chars) — keeps first 40%, last 60%
  max_lines: 2000 # read_file pagination cap
  max_line_length: 2000 # Per-line cap in read_file view
```

When terminal output exceeds `max_bytes`, truncation inserts `[OUTPUT TRUNCATED]` notice.

**Per-model tuning:**

```yaml
# Large context model (200K+)
tool_output:
  max_bytes: 150000
  max_lines: 5000

# Small local model (16K context)
tool_output:
  max_bytes: 20000
  max_lines: 500
```

### 6. File Read Safety

```yaml
file_read_max_chars: 100000 # ~25-35K tokens — max per read_file call
```

Exceeded reads return error telling agent to use offset/limit. Auto-deduplicates repeated reads.

```yaml
# Large context model
file_read_max_chars: 200000

# Small local model
file_read_max_chars: 30000
```

### 7. Terminal Backend Setup

```yaml
terminal:
  backend: local # local | docker | ssh | modal | daytona | singularity
  cwd: "."
  timeout: 180 # Per-command timeout
  persistent_shell: true # Keep state across commands (SSH default: true)
  env_passthrough: [] # Env vars to forward
```

**Each backend requires specific setup:**

```bash
# Docker
hermes config set terminal.backend docker
docker version  # Verify

# SSH
export TERMINAL_SSH_HOST=my-server.example.com
export TERMINAL_SSH_USER=ubuntu

# Modal
export MODAL_TOKEN_ID=...
export MODAL_TOKEN_SECRET=...

# Daytona
export DAYTONA_API_KEY=...

# Singularity
# apptainer in PATH (HPC clusters)
```

### 8. Browser Backend Configuration

Browser supports multiple backends:

| Backend            | Description                   |
| ------------------ | ----------------------------- |
| Playwright (local) | Built-in browser automation   |
| Browserbase        | Cloud browser (needs API key) |
| Nous Portal        | Browser through Tool Gateway  |
| MCP browser server | Via MCP integration           |

```bash
hermes tools  # Interactively set browser backend
```

### 9. Web Tool Configuration

Web search/extract backends:

| Backend     | Key needed                |
| ----------- | ------------------------- |
| Firecrawl   | `FIRECRAWL_API_KEY`       |
| Nous Portal | OAuth via `hermes portal` |
| Tavily      | `TAVILY_API_KEY`          |
| DuckDuckGo  | None (fallback)           |

Set via `hermes tools` or config:

```yaml
web:
  backend: nous # nous | firecrawl | tavily | duckduckgo
```

### 10. Code Execution Setup

```yaml
# In config.yaml
# execute_code runs in same sandbox as terminal
# No separate config needed — inherits terminal backend
```

Code execution (`execute_code`) tool:

- Runs Python, shell scripts
- Shares sandbox with terminal backend
- Env vars filtered for security (blocks `KEY`, `TOKEN`, `SECRET`, `PASSWORD`)
- Skill-declared `required_environment_variables` auto-passthrough

### 11. Dangerous Command Approval for Tools

```yaml
approvals:
  mode: manual # manual | smart | off
  timeout: 60 # Approval timeout (seconds)
  cron_mode: deny # deny | approve — cron behavior
```

Approval flow checks all terminal commands against dangerous patterns before execution.

Container backends (docker, modal, etc.) **skip** dangerous checks — container is the security boundary.

### 12. Tool Display Settings (CLI)

```yaml
display:
  tool_progress: all # off | new | all | verbose — tool activity display
  tool_progress_command: true # Enable /verbose in messaging
  tool_preview_length: 0 # Max chars in tool preview (0 = unlimited)
```

Toggle in session:

```text
/verbose     # Cycle: off → new → all → verbose
```

### 13. Media Tools (Image, TTS)

```yaml
image_gen:
  provider: nous # nous | openai | fal | ...

tts:
  provider: nous # nous | openai | elevenlabs | ...
```

Config via `hermes tools`.

### 14. Tool Security Features

- **SSRF protection**: All URL tools block private IPs, loopback, cloud metadata
- **Website blocklist**: Restrict domains agent can access
- **Tirith pre-exec scanning**: Detects homograph URLs, pipe-to-interpreter, injection
- **Credential filtering**: Env vars with `KEY`, `TOKEN`, `SECRET`, `PASSWORD` blocked from `execute_code`
- **MCP env filtering**: Only safe system vars + explicit `env` passed to MCP subprocesses

## Pitfalls

- **`disabled_toolsets` applies after per-platform config**: Toolsets removed everywhere even if platform allows
- **Container backends skip dangerous checks**: No approval prompts for rm/drop/etc.
- **Tool output limits affect reasoning**: Truncated output may hide important details. Raise for large-context models
- **`file_read_max_chars` too low**: Agent can't read larger files. Error tells agent to use offset/limit
- **Browser backends need separate config**: Playwright auto, Browserbase needs key, Portal needs OAuth
- **Code execution shares terminal sandbox**: Same env vars, same working directory
- **Per-platform config persisted by `hermes tools`**: Edit manually if needed, but `hermes tools` overwrites
- **`tool_progress: off` in messaging = no breadcrumbs**: User sees only final response
- **Website blocklist affects all URL tools**: Not just web search — browser and vision too

## Verification

```bash
hermes tools                                        # Interactive tool config
hermes config show | grep -A 5 tool_output           # Check limits
hermes config show | grep -A 5 agent.disabled_toolsets  # Global disables
# In session:
/tools                                              # List available tools
/verbose                                            # Toggle display mode
```
