---
name: hermes-memory-personality
description: Manage Hermes memory (MEMORY.md, USER.md) and personality (SOUL.md, /personality) — persistence, capacity, context files, identity
version: 1.0.0
platforms: [linux, macos, windows]
metadata:
  hermes:
    tags: [memory, personality, soul, context, profile]
    category: productivity
    requires_toolsets: [terminal]
---

# Hermes Memory & Personality

> **Tentaflake context:** This skill describes Hermes' built-in memory system
> — it works the same inside tentaflake agent containers. The memory settings
> (`memory.memory_enabled`, `memory.user_profile_enabled`, etc.) can be
> configured declaratively via the `settings` attrset in `my-agents.nix`.
> See `my-agents.nix.example` for examples. Agent state (including memory)
> persists in `/var/lib/hermes-<name>/` on the host.

## When to Use

- Manage what the agent remembers across sessions
- Configure SOUL.md personality
- Set up AGENTS.md project context files
- Control memory capacity and consolidation
- Switch personalities mid-session
- Understand memory vs skills distinction
- Use context files (CLAUDE.md, .cursorrules)

## Procedure

### 1. Memory System Overview

Two files persist across sessions:

| File          | Purpose                                         | Char Limit                | Location                       |
| ------------- | ----------------------------------------------- | ------------------------- | ------------------------------ |
| **MEMORY.md** | Agent's notes — env facts, conventions, lessons | 2,200 chars (~800 tokens) | `~/.hermes/memories/MEMORY.md` |
| **USER.md**   | User profile — preferences, style, expectations | 1,375 chars (~500 tokens) | `~/.hermes/memories/USER.md`   |

Both inject into system prompt as frozen snapshot at session start.

### 2. Memory Tool Actions

Agent uses `memory` tool:

```text
memory(action="add", target="memory", content="User prefers TypeScript over JavaScript")
memory(action="replace", target="memory",
       old_text="TypeScript",
       content="User prefers TypeScript over JavaScript")
memory(action="remove", target="memory", old_text="TypeScript")
```

| Action    | Use             | Key params                                  |
| --------- | --------------- | ------------------------------------------- |
| `add`     | New entry       | `target` (`memory` / `user`), `content`     |
| `replace` | Update existing | `target`, `old_text` (substring), `content` |
| `remove`  | Delete entry    | `target`, `old_text` (substring)            |

No `read` — memory auto-injected in system prompt. `old_text` uses substring matching. If multiple matches → error asking for more specific text.

### 3. Memory vs Skills

| Feature    | Memory                          | Skills                           |
| ---------- | ------------------------------- | -------------------------------- |
| Stores     | Facts ("what")                  | Procedures ("how")               |
| Trigger    | Auto-injected in prompt         | On-demand via `/skill-name`      |
| Capacity   | ~1,300 tokens total             | Unlimited (files on disk)        |
| Management | Agent curated via `memory` tool | Agent created via `skill_manage` |
| Token cost | Fixed per session               | Variable (loaded when needed)    |

**Memory = facts** (environment, preferences). **Skills = procedures** (multi-step workflows).

### 4. Capacity Management

**Char limits:**

| Store  | Limit       | Typical entries |
| ------ | ----------- | --------------- |
| memory | 2,200 chars | 8-15 entries    |
| user   | 1,375 chars | 5-10 entries    |

**When memory is full**, add returns error with current entries. Agent should:

1. Read current entries
2. Remove or consolidate
3. Replace merged entries (shorter)
4. Re-add new entry

**Best practice**: Above 80% capacity, consolidate before adding. Merge related facts.

**Good entries:**

```text
# Packs multiple facts
User runs macOS 14 Sonoma, uses Homebrew, has Docker Desktop. Shell: zsh with oh-my-zsh.

# Specific convention
Project ~/code/api uses Go 1.22, sqlc for DB queries, chi router. Run tests with 'make test'.
```

**Bad entries:**

```text
# Too vague
User has a project.

# Too verbose
On January 5th, 2026, the user asked me to look at...
```

### 5. Memory Config

```yaml
# ~/.hermes/config.yaml
memory:
  memory_enabled: true
  user_profile_enabled: true
  memory_char_limit: 2200
  user_char_limit: 1375
  write_approval: false # false=write freely | true=require approval
```

**`write_approval: true`** gates all saves. Use `/memory` commands in session:

```text
/memory pending             # List staged writes
/memory approve <id>        # Apply one (or 'all')
/memory reject <id>         # Drop one (or 'all')
/memory approval on         # Turn gate on/off
```

### 6. Session Search vs Memory

| Feature  | Memory                     | Session Search      |
| -------- | -------------------------- | ------------------- |
| Capacity | ~1,300 tokens              | Unlimited           |
| Speed    | Instant (in prompt)        | ~20ms FTS5 query    |
| Cost     | Token cost every prompt    | Free                |
| Use case | Key facts always available | "Did we discuss X?" |

Agent uses `session_search` tool to find past discussions.

### 7. SOUL.md Personality

**Location:** `~/.hermes/SOUL.md` (or `$HERMES_HOME/SOUL.md`)

**Properties:**

- Slot #1 in system prompt — agent identity
- Auto-seeded if missing (never overwritten)
- Loaded from `HERMES_HOME` only (not CWD)
- Security-scanned before injection
- If empty → falls back to built-in default identity

**Good SOUL.md:**

```markdown
You are a pragmatic senior engineer with strong taste.
You optimize for truth, clarity, and usefulness over politeness theater.

## Style

- Be direct without being cold
- Prefer substance over filler
- Push back when something is a bad idea

## What to avoid

- Sycophancy
- Hype language
- Overexplaining obvious things
```

**SOUL.md vs AGENTS.md:**

| File      | Purpose                           | Scope                 |
| --------- | --------------------------------- | --------------------- |
| SOUL.md   | Identity, tone, style             | Global (all projects) |
| AGENTS.md | Project instructions, conventions | Per-project (CWD)     |

### 8. Built-in Personalities

Switch with `/personality <name>`:

| Name        | Description                 |
| ----------- | --------------------------- |
| helpful     | Friendly, general-purpose   |
| concise     | Brief, to-the-point         |
| technical   | Detailed, precise           |
| creative    | Innovative, outside-the-box |
| teacher     | Patient educator            |
| kawaii      | Cute expressions            |
| pirate      | Captain Hermes              |
| shakespeare | Bardic prose                |
| noir        | Hard-boiled detective       |

Custom personalities in config:

```yaml
agent:
  personalities:
    codereviewer: >
      You are a meticulous code reviewer. Identify bugs, security issues,
      performance concerns. Be precise and constructive.
```

Switch with `/personality codereviewer`.

### 9. Context Files

| File                       | Purpose                                 | Discovery            |
| -------------------------- | --------------------------------------- | -------------------- |
| `.hermes.md` / `HERMES.md` | Project instructions (highest priority) | Walks to git root    |
| `AGENTS.md`                | Conventions, architecture               | CWD + subdirectories |
| `CLAUDE.md`                | Claude Code context                     | CWD + subdirectories |
| `SOUL.md`                  | Global personality                      | `HERMES_HOME` only   |
| `.cursorrules`             | Cursor IDE rules                        | CWD only             |

**Priority**: Only one project context type loaded per session: `.hermes.md` → `AGENTS.md` → `CLAUDE.md` → `.cursorrules`. SOUL.md always loaded independently.

**Progressive subdirectory discovery**: As agent navigates into subdirectories, nested `AGENTS.md` files are discovered and injected into tool results.

**Security scan**: All context files scanned for prompt injection (ignore instructions, hidden HTML, credential exfiltration, invisible Unicode). Blocked files show `[BLOCKED]`.

**Size limits**: 20,000 chars max per file (70% head, 20% tail truncation). Subdirectory context files capped at 8,000 chars.

### 10. Memory Persistence Details

- **Frozen snapshot**: Memory loads at session start, never changes mid-session (preserves prompt cache)
- **Writes persist immediately**: Agent writes to disk in real-time. Changes visible in next session
- **`session_search`**: Agent can find past conversations via FTS5 full-text search
- **External memory providers**: Honcho, Mem0, Hindsight, etc. for semantic search and knowledge graphs

```bash
hermes memory setup      # Pick external provider
hermes memory status     # Check active provider
```

### 11. Recommended Workflow

1. Set global SOUL.md for stable identity
2. Put project instructions in AGENTS.md per repo
3. Use `/personality` for temporary mode shifts
4. Let agent auto-save memories (or set `write_approval: true` for gating)
5. Use `/compress` to manage session length
6. Ask agent to "remember this for next time" after productive sessions

## Pitfalls

- **Memory is frozen snapshot**: Mid-session changes don't appear in system prompt until next session
- **Memory char limits strict**: Agent must consolidate when full. Over-limit adds return error
- **Substring matching needs uniqueness**: If `old_text` matches multiple entries → error
- **Duplicate entries auto-rejected**: Exact duplicates return success with "no duplicate added"
- **SOUL.md not loaded from CWD**: Only from `HERMES_HOME` — personality stays consistent across projects
- **Context files scanned for injection**: Malicious files blocked. Review files from untrusted repos
- **Memory vs skills confusion**: Use memory for facts, skills for procedures. Skills persist via `skill_manage`
- **External memory providers run alongside**: Never replace built-in memory
- **`write_approval` gates foreground + background**: Background writes staged for review

## Verification

```bash
hermes config show | grep -A 10 memory    # Check memory config
cat ~/.hermes/SOUL.md                      # View personality
cat ~/.hermes/memories/MEMORY.md           # View current memory
cat ~/.hermes/memories/USER.md             # View user profile
# In session:
/memory pending                            # Check staged writes
```
