# Pi Durable integration brief

Status: proposed integration, not runnable support. The original agent-only
brief has been superseded by the requested whole-system design:

1. [System architecture and UI/state ownership](system-architecture-plan.md)
2. [Threat model and security acceptance scenarios](security-threat-model-plan.md)
3. [Ordered implementation backlog and verification matrix](implementation-backlog.md)
4. [Autonomous implementation goal](autonomous-implementation-goal.md)

Use the implementation goal only when the user explicitly starts that work.
This document is not authorization for implementation, deployment or activation.

## Durable-specific requirements retained

- Extend the existing `pi` adapter; its current version label is not SDK or
  runtime acceptance. Review and pin the actual artifact, runtime and API.
- Keep a thin upstream TypeScript runner; prefer Rust for surrounding schema,
  API/control, connector and observability integration. Do not replace the model
  loop or fork the scheduler to avoid an unsupported recovery contract.
- Use one protected persistent SQLite writer per admitted instance and stable
  operation IDs. Test graceful stop, crash, storage/migration failure and restore.
- Wire actual tools through exact host model/Research/worker boundaries. No
  unrestricted native bash, direct web/provider egress or host-control socket.
- First prove the pinned SDK can prevent automatic repeat of uncertain model,
  Research or worker dispatches, including restart recovery. If not, keep the
  scaffold non-runnable and report the precise upstream blocker.
- Board owns linked tasks; Durable owns native execution/conversations. Persist
  explicit correlations and permitted report/source provenance; do not create
  two editable task stores or claim completed work from a status alone.
- The future public source is `tentaflake.toml`, replacing `agents.toml` through
  an explicit migration and legacy removal. Existing runtime behavior remains
  governed by current source until that implementation is delivered.
- An optional per-host security analyst is advisory and budgeted. It does not
  control containers, approve policy, handle credentials or provide reliable
  prompt-injection detection. Human-reviewed deterministic interfaces act.
- Backup consistency, no-replay receipts, cost state, optional UI and human-only
  activation remain separate requirements, not features granted by durability.

Upstream starting points, to be checked at a selected immutable revision:
[announcement](https://earendil.com/posts/pi-durable/),
[README](https://github.com/earendil-works/pi/blob/main/packages/durable/README.md),
[Pi documentation](https://pi.dev/docs/latest).
