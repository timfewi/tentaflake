# Bundled skills

Skills are procedural references, not permissions or runtime integrations.
The shared collection in `.agents/skills/` is runtime-agnostic:

| Skill | Purpose |
|---|---|
| [handle-the-host](../.agents/skills/handle-the-host/SKILL.md) | Authorized private management and maintenance |
| [tentaflake-change-review](../.agents/skills/tentaflake-change-review/SKILL.md) | Requirements, evidence and PR review |
| [tentaflake-repo-guidance](../.agents/skills/tentaflake-repo-guidance/SKILL.md) | Owning sources and guides |

Keep skills short: trigger, actionable procedure, boundaries and verification.
Frontmatter needs a directory-matching `name` and task-specific `description`.
Link authoritative guides rather than duplicate option tables or security
policy. Update relevant skills with behavior changes; never include deployment
identities, private context or credentials.

## Optional seeding

The public `tentaflake.toml` schema does **not** accept `seedDir`, arbitrary
native settings or private agent context. Internal adapter builders may support
`seedDir` for reviewed public fixtures or trusted internal composition; this is
not an additional public configuration API. Where used, seeding copies in the
adapter's native layout without overwriting existing files; changed seeds do
not synchronize runtime state. Verify adapter paths and ownership first.
Repository-relative guide links need adapting to the deployment's pinned docs;
copying a skill alone does not install the repository or grant its tools.

Community skills require source review and a permitted acquisition path. A
balanced capsule has no general download route. Prepare reviewed public files
outside it; keep private memories/personality in a separate runtime channel.
See [native state](agent-adapters.md#native-configuration-and-state) and
[documentation ownership](18-documentation.md).
