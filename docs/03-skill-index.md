# NixOS Agent Orchestration — Skill Index

Hermes skills are procedural knowledge packs that extend the agent's
capabilities. Unlike memory (facts), skills are **how-to** — multi-step
workflows loaded on demand.

---

## What Are Hermes Skills?

- **Skills** = procedures ("how to configure a provider", "how to debug a gateway")
- **Memory** = facts ("user prefers Claude", "the API key is in /run/secrets/")

Skills live as `SKILL.md` files. The agent loads them automatically from
`$HERMES_HOME/skills/` at startup. Load on demand with:

```text
/skill-name
```

Or from the CLI:

```bash
hermes skills list
hermes skills load skill-name
```

---

## Bundled Skills

These skills ship with the installed system at
`/etc/nixos/skills/` (copied to each agent's `$HERMES_HOME/skills/` during
install).

| Skill | Purpose | Category |
|-------|---------|----------|
| **hermes-config-manager** | Config structure, profiles, env vars, `hermes config set` | devops |
| **hermes-provider-setup** | LLM provider setup, model selection, OpenRouter/Nous/Anthropic | devops |
| **hermes-memory-personality** | MEMORY.md, USER.md, SOUL.md — identity and persistence | productivity |
| **hermes-tools-config** | Tool/toolset enable/disable, backends, truncation | devops |

### Skill details

**hermes-config-manager**
- Config directory layout (`~/.hermes/`)
- Profile management (`hermes profile create`, `hermes -p work chat`)
- Terminal backend switching (local/docker/ssh/modal)
- Config migration after update

**hermes-provider-setup**
- Provider selection guide (Nous Portal, OpenRouter, Anthropic, OpenAI, etc.)
- API key configuration
- Multi-provider fallback
- Custom endpoints (Ollama, vLLM, SGLang)
- Model selection and switching mid-session

**hermes-memory-personality**
- MEMORY.md vs USER.md — what to store where
- Memory tool actions (add, replace, remove)
- Capacity limits (2,200 chars memory, 1,375 chars user)
- SOUL.md personality configuration
- Context files (CLAUDE.md, AGENTS.md)

**hermes-tools-config**
- Tool vs Toolset distinction
- Per-platform tool enable/disable (CLI, Telegram, Discord, Slack)
- Browser backend configuration
- Code execution sandboxing
- Output truncation limits

---

## How Skills Auto-Discover

Agents scan `$HERMES_HOME/skills/` at startup. Each `SKILL.md` file in
that directory is registered. The agent can then invoke them via:

```text
Use the hermes-config-manager skill to set up my model provider.
```

No manual import needed. Drop a `SKILL.md` into the agent's skills dir and
restart:

```bash
sudo docker restart hermes-coding
```

---

## Available via Taps (Installable)

These skills are **not bundled** but available from community taps.
Install them at runtime.

| Skill | Purpose | Category |
|-------|---------|----------|
| **hermes-debug-diagnose** | Doctor, provider connectivity, session recovery | devops |
| **hermes-gateway-setup** | Telegram/Discord/Slack/WhatsApp/Signal | devops |
| **hermes-mcp-integration** | MCP server config, tool filtering, OAuth | devops |
| **hermes-security-hardening** | Approval modes, sandboxing, SSRF protection | security |
| **hermes-skill-author** | Create new skills, test, bundle | productivity |
| **hermes-skill-tap-publisher** | Publish skill taps to GitHub | devops |

Install via:

```bash
# Inside the agent container
hermes skills install hermes-debug-diagnose
hermes skills install hermes-gateway-setup
hermes skills install hermes-mcp-integration
hermes skills install hermes-security-hardening
hermes skills install hermes-skill-author
hermes skills install hermes-skill-tap-publisher
```

Or by tap URL:

```bash
hermes skills install https://github.com/username/hermes-skill-tap
```

### Skill reference

**hermes-debug-diagnose**
- `hermes doctor` — full system health check
- Provider error diagnosis (400, 401, 429, empty responses)
- Gateway connectivity tests
- Session resume and recovery

**hermes-gateway-setup**
- Telegram bot token setup
- Discord bot configuration
- Slack app integration
- WhatsApp business API
- User allowlists and DM pairing
- Cron job management

**hermes-mcp-integration**
- Add MCP servers (GitHub, filesystem, Stripe, etc.)
- Tool allow/block filtering
- OAuth-authenticated MCP servers
- mTLS configuration

**hermes-security-hardening**
- Dangerous command approval mode
- Container sandboxing (Bubblewrap)
- SSRF protection
- Tirith security scanning
- MCP tool filtering

**hermes-skill-author**
- SKILL.md anatomy (frontmatter, sections)
- Config, env vars, ref files
- Skill testing
- Bundle packaging

**hermes-skill-tap-publisher**
- GitHub tap repo layout
- Trust levels (official, verified, community)
- Update lifecycle
- Security scanning

---

## Finding More Skills

```bash
# Search official skill hub
hermes skills search web-automation
hermes skills search gateway
hermes skills search security

# List installed
hermes skills list
```

Community sources:
- GitHub topic: `hermes-agent-skill`
- Nous Research Discord skill-sharing channels
- Self-published taps (GitHub repos with skill bundles)

---

## Creating Your Own Skills

Minimal `SKILL.md`:

```markdown
---
name: my-custom-task
description: Do a specific thing
version: 1.0.0
metadata:
  hermes:
    tags: [custom, automation]
    category: productivity
---

# My Custom Task

## Procedure

### 1. First step

```bash
some command
```

### 2. Second step

Check output, do thing.
```

Place it in the agent's skills dir:

```bash
sudo mkdir -p /var/lib/hermes-coding/skills/my-custom-task
sudo vi /var/lib/hermes-coding/skills/my-custom-task/SKILL.md
sudo docker restart hermes-coding
```

Use the `hermes-skill-author` skill for advanced authoring, testing,
and packaging.

---

## External References

The following are **dev-machine-only** reference docs (not on the target
machine — stored on the development workstation):

- **`~/docs/qp-hermes-analyze.md`** — Quick-profile analysis workflow
- **`~/docs/agency-inbox-operator.md`** — Multi-agent inbox orchestration

These describe dev workflows, not operational procedures. They live on the
machine where you fork this repo, not on the deployed agent host.
