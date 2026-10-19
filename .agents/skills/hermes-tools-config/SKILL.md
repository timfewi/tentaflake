---
name: hermes-tools-config
description: Adjust Hermes tools and output limits without bypassing Tentaflake's Research or disposable-worker policy.
version: 1.1.0
platforms: [linux]
metadata:
  hermes:
    tags: [tools, toolsets, research, execution]
    category: devops
    requires_toolsets: [terminal]
---

# Hermes tools

1. Resolve the pinned instance and inspect its actual tool catalog/help.
   Toolsets, platform presets and output options vary by image.
2. Change declarative `settings` through
   [configuration](../hermes-config-manager/SKILL.md). Interactive `hermes tools`
   may write configuration and is unsuitable for generated read-only config.
3. Keep Research on `mcp_servers.secure-research-tool`. The adapter adds native
   `web` and `browser` to `agent.disabled_toolsets` after caller settings.
   Additional MCP entries must be local stdio commands, never remote URLs.
   Preserve exact read-only client/socket mounts; do not add direct web,
   cloud-browser, provider-hosted tools or legacy fetch.
4. Route untrusted execution to the offline worker queue. A worker declaration
   does not intercept Hermes terminal/code tools automatically. Alternate
   Docker, SSH or cloud backends are not accepted worker mediation.
5. Keep output/file reads bounded. Verify native option names and units;
   prefer scoped/paginated output before increasing limits.

## Verify routing

After an authorized application, check discovered tools and one permitted
Research request or worker job as applicable. Confirm offline execution,
exact-ID results and disabled alternate network tools. Distinguish synthetic
fixture evidence from an actual vendor session.

Tool visibility and upstream command approval do not grant host authority or
prove containment. Worker approval adds no network, secrets or host privileges.
See [Research](../../../docs/16-research.md),
[worker controls](../../../docs/13-disposable-worker.md) and
[adapter support limits](../../../docs/agent-adapters.md).
