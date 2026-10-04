---
name: hermes-memory-personality
description: Maintain one Hermes instance's memory, personality and seeds without overwriting runtime state or exposing private context.
version: 1.1.0
platforms: [linux]
metadata:
  hermes:
    tags: [memory, personality, soul, context]
    category: productivity
    requires_toolsets: [terminal]
---

# Hermes memory and personality

Resolve the effective state path and numeric owner from declaration/inventory.
Hermes uses `$HERMES_HOME`, normally `/var/lib/hermes-<name>`, with UID/GID
10000 by default. Do not edit the operator's `~/.hermes` or another instance.

| Content | Usual path under `$HERMES_HOME` |
|---|---|
| Identity and tone | `SOUL.md` |
| Durable facts/preferences | `memories/MEMORY.md`, `memories/USER.md` |
| Reusable procedures | `skills/<name>/SKILL.md` |

Confirm native paths, limits and reload/session behavior against the pinned
image. Memory/personality commands and context precedence are version-dependent;
do not assume a fixed catalog or token limit.

1. Inspect only requested content. Private memories, identities and secrets
   stay out of the generic template.
2. Keep facts in memory, procedures in skills and stable tone in SOUL.
   Consolidate stale/duplicate facts while preserving unrelated entries.
3. Use the supported memory tool or a focused edit with correct ownership.
   Avoid concurrent-write races; arrange an authorized stop if required.
4. Set native memory options through declarative `settings`; generated config
   is read-only. External memory providers need a separately reviewed
   data/credential/network path.
5. Seed initial non-secret `SOUL.md`, `memories/` or `skills/` through a
   consumer-owned `seedDir`. No-clobber copying preserves existing files:
   changed seeds do not synchronize runtime state. Seeds enter the Nix store;
   private material needs a runtime channel.

Verify the changed file and permissions without dumping other context.
Use a fresh session or verified reload and check the requested behavior.
Restart/restore drills require their own authorized scope. Back up state and
respect mount boundaries; see [agent management](../../../docs/02-agent-tips.md),
[bundled skills](../../../docs/03-skill-index.md) and
[backup/restore](../../../docs/07-operations.md).
