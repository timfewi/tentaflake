# Tentaflake system architecture plan

Status: proposed design, not implemented support or deployment evidence.
This plan replaces the agent-only scope of the earlier
[Durable brief](pi-durable-implementation-prompt.md). Implementation order and
verification are in the [backlog](implementation-backlog.md); boundaries are in
the [security plan](security-threat-model-plan.md). The separately executable
[implementation goal](autonomous-implementation-goal.md) does not start itself.

The backlog's open-point register O01-O09 is mandatory implementation and
acceptance scope. Runtime opt-in and delivery priority do not make identified
gaps optional; unresolved operator/upstream dependencies remain incomplete.

## Product and acceptance journey

Tentaflake is a declarative, runtime-agnostic NixOS framework for isolated,
24/7 agents. A user should not need to write Nix:

1. Install a reviewed baseline, with disk selection and destructive actions
   explicitly confirmed by the human; or adopt an existing NixOS consumer.
2. Open the private, authenticated operator UI; the CLI offers the same actions.
3. Describe or edit the desired system, inspect a plain-language plan and exact
   `tentaflake.toml` diff, then approve that revision.
4. Validate, generate internal Nix and build the approved system; inspect progress,
   errors and the immutable output. Build and activation are separate operations.
5. The operator activates the reviewed result. An agent never performs activation.
6. Start an admitted, already-declared container, run a board-linked research task,
   inspect its results/sources, and watch Hostpulse and policy/tool activity.

The UI must offer forms as well as a prompt. Model availability is not required
for configuration, lifecycle control, monitoring or recovery. No UI is mandatory
for operating Tentaflake. "Rebuild" means validation and build; do not hide host
activation behind that label or give the browser arbitrary root commands.

## Source baseline and gaps

These are local source findings, not checks of an activated host. References to
other repositories use repository name plus repository-relative path; no private
host identity, endpoint, credential or telemetry is copied here.

| Owner | Inspected baseline | Present implementation and missing integration |
| --- | --- | --- |
| Tentaflake | `6e1aa7965a25a1e071d79b91c3ac9742a6adb4e1` | Rust CLI/broker/worker, Nix policy, optional Python Unix host API. `agents.toml` is current authority. `adapters/pi.nix` is a refusal scaffold. No system-wide TOML compiler or accepted Pi runtime. |
| tentaflake-ui | `4f57a15b76d0713c2352e92a3044d4fa46225345` | Static Next/React/TypeScript UI and Python bridge; same-origin HTTP/SSE, private SQLite, Pi run adapters and opt-in HQ task authority. Settings is not a system editor. Hostpulse integration is absent. |
| team-kanban-board | `f09e3b254a962c52c115a458e308450b95eff934` | Bun/TypeScript, React, SQLite, HTTP/SSE, research assignment/report APIs. Issues and research runs are separate. No admitted production Pi worker. No root license or manifest license declaration found; reuse/publication needs clarification. |
| Hostpulse | `94b21bb6b9e1556eaf3d6074204ba09b5af38bc4` | Rust collector/archive/CLI, private same-UID socket and separate read-only Tauri UI. Host/cgroup metrics exist; network accounting and container discovery do not. Nix module is user-session-only, not a 24/7 system collector. An existing staged store-quota edit must be preserved. |
| tentaflake-research | `6cf3a278a292b4ae11fc250d27aded5c4f81e525` | Rust public-web Research fork, five tools, framed protocol v1, authenticated readiness and per-agent relays. Not an enterprise-data connector. |
| nixos-host | `9db6326033d41c0bb039744b762b8715d3070737` plus unrelated local changes | Existing private consumer declares a board service and private HTTPS; Hostpulse package input exists but is not VPS collector admission. No Tentaflake/UI host composition was found in the inspected consumer entrypoint. |

Source entrypoints:

- Tentaflake: [adapter](../adapters/pi.nix), [parser](../lib/agentsFromData.nix),
  [adapter contract](../lib/adapterContract.nix),
  [host API](19-web-ui.md), [quota implementation](../modules/workspace-quota.nix),
  [installer](00-install.md), [operations](07-operations.md).
- UI: `bridge/config.py`, `bridge/server.py`, `bridge/transport.py`,
  `bridge/jobs.py`, `bridge/store.py`, `docs/api.md`, `components/app-shell.tsx`.
- Board: `src/shared.ts`, `src/server/research.ts`, `src/server/db.ts`,
  `src/server/app.ts`, `nix/module.nix`, `package.json`.
- Hostpulse: `crates/hostpulse-core/src/config.rs`,
  `crates/hostpulse-protocol/src/{lib,transport}.rs`,
  `crates/hostpulse-linux/src/metrics.rs`, `nix/module.nix`, `README.md`.
- Research: `src/protocol.rs`, `src/mcp.rs`, `nix/module.nix`,
  `nix/container-clients.nix`, `docs/api.md`, `UPSTREAM.md`.
- Host: `flake.nix`, `hosts/vps/{default,storage}.nix`, `tests/vps.nix`.

### VPS evidence boundary

An authorized read-only Tailscale SSH inventory attempt failed during host-name
resolution before connecting. CPU, RAM, free disk, actual filesystem, container
engine, gVisor, active services and listeners remain **unknown**. No installation,
service change or provider request occurred. Repairing DNS/Tailnet configuration
requires operator action; do not bypass the failed route or change enrollment.

The consumer source declares ext4 root. Quota code creates separate Btrfs loop
images; it does not imply that the VPS root must be reformatted. Validate image
backing, kernel support, free space and mount behavior on a synthetic ext4-backed
host before deployment. Do not convert the real root or hide existing directories.
The host orientation card was stale for changed flake inputs; source was read
instead. Existing host edits and the staged Hostpulse quota edit are not this plan.

## Ownership and dependency direction

```text
private consumer host configuration
  -> Tentaflake modules + approved tentaflake.toml + hardware/storage facts
  -> optional independent UI, board, Hostpulse and Research packages/modules

operator UI / CLI
  -> authenticated Rust application API (read/model/task projections)
  -> separate narrow host management / build interfaces
  -> board authority, Hostpulse read projection, per-agent runtime interface

isolated Pi Durable container
  -> exact host model broker
  -> exact Research relay / separately authorized enterprise connector relay
  -> offline worker queue for mediated execution
```

Hostpulse and the board remain independent products with their own configuration,
protocol and state. They must not import Tentaflake to function. Integration is
configuration-driven in Tentaflake/consumer adapters; no circular flake inputs.
Stateless adapters mean no duplicate source of truth, not that conversations,
telemetry, approvals or board records can be discarded. Do not link a repository's
private workspace or runtime databases into another product's source/build.

### Rust-first boundaries

- New schema/parser/validator, CLI logic, API bridge, build coordinator, connector
  gateway and telemetry projections: Rust, using reviewed pinned dependencies.
- Existing Rust broker/worker/Research/Hostpulse: extend their owning contracts.
- Pi Durable: a thin TypeScript runner around the pinned upstream JS harness.
  Do not port the model loop/scheduler or pretend Rust removes this dependency.
- Nix remains module/build language. Keep the existing browser TypeScript/React
  frontend; a Rust HTTP backend does not require rewriting it in WASM.
- Port Python UI/host API routes behind golden HTTP/SSE/SQLite contract fixtures;
  migrate all known callers per cutover and remove replaced implementations.
  Do not simultaneously rewrite the standalone Bun board just for language purity.
  A later board backend port requires its own contract-preserving justification.

## One declarative source: tentaflake.toml

`tentaflake.toml` replaces `agents.toml`; HTTP uses JSON from the same typed model.
Generate standard internal `flake.nix`/modules, not a new `tenta.flake` language.
No arbitrary Nix, executable snippets, remote module URLs or shell interpolation
in the public schema. Explicit schema version, unknown-field rejection, finite
JSON-compatible values, bounded size/cardinality and stable normalized identities.

| Section | Scope and enforcement |
| --- | --- |
| system | Schema version, supported platform and reviewed baseline/profile. Hardware facts remain consumer-owned. |
| network | Approved private-management and broker/Research/connector modes; not arbitrary firewall or Tailnet administration. |
| storage | State/workspace capacities, retention and backup selection. No disk formatting or implicit data relocation. |
| agents | Runtime/template identity, immutable artifact, declared stopped instances, model/tool capabilities and resource bounds. |
| sandbox | Existing security profile, worker/approval requirements and reviewed isolation settings. Cannot downgrade host policy. |
| shell | Approved interactive preferences/tool profiles, not root commands or startup scripts. |
| llm | Model/provider policy references, budgets and time/concurrency limits. No literal keys. |
| container | Supported engine selection and reviewed resource/profile defaults, not privileged mounts or engine sockets. |
| ui / board | Optional interface selection, endpoint identity references and one external board authority. |
| monitoring | Source consent, workload mappings, freshness and retention within operator ceilings. |
| research / connectors | Separate explicitly granted capability sets and runtime credential references. |
| securityObserver | Disabled-by-default advisory analyst, input fields, model budget and review workflow. |

This table is the proposed v1 scope, not accepted field syntax. Publish the typed
schema and synthetic examples before building forms. Sensitive installation
facts and unrestricted host policy remain outside this public data model.
Unsupported categories/values fail visibly; never accept a field and ignore it.

Use a Rust schema owner with exported JSON schema/types and cross-language golden
fixtures. Nix loading must agree with Rust semantic validation; compilation uses
typed serialization rather than concatenating untrusted source. Generated native
service files are projections, never independent configuration authorities.
Runtime-only secret resolution happens after the public build, not through Nix
reading credential files. Hash canonical semantic data separately from editable
TOML text; preserve comments where possible and never lose unknown input silently.

### Explicit migration, then delete legacy

Offline migration reads a selected legacy inventory and current declared host
policy, emits a candidate TOML and a field-by-field preservation/conflict report.
It never evaluates an untrusted legacy Nix file to discover policy. Unsupported
host settings require explicit mapping; migration is not permission to invent
network, credentials, quotas or empty inventories. Preserve IDs, numeric owners,
state/workspace paths, receipts, model budgets and stopped/startup intent.

Validate and compare the generated inventory/policy before switching sources.
Retain a protected backup outside the live configuration path. After cutover,
remove `agents.toml`, its production parser/API, installer templates, CLI callers,
fixtures and obsolete docs in the same migration series. No dual reader,
compatibility alias or silent fallback. Migration artifacts/tests may mention the
old format; they must not become a runtime compatibility mode. UI JSON configuration
becomes a generated private projection where needed, not a second editable source.
Do not delete unrelated HQ/Hermes application state as an incidental config change.

## UI/CLI and host build state machine

Use one versioned application contract for both clients. Separate capabilities:
view, propose/edit, approve-build, lifecycle-manage, connector-write-approve and
operator activation. An editable UI is not a generic host shell. Existing single
operator tokens are not enterprise RBAC; multi-user deployment remains blocked
until authenticated identity and resource-scoped authorization are implemented.

```text
natural-language request / forms / CLI
  -> draft -> typed validation -> semantic diff + warnings
  -> approval(config hash, base revision, compiler/policy/input revisions)
  -> bounded evaluation -> queued build -> building -> built | failed | unknown
  -> operator handoff -> independently observed activated | not activated | unknown
```

Drafts and status receipts live in private application state. Only the approved
canonical TOML becomes declarative input. CSRF/origin controls, optimistic revision
checks, idempotency keys, bounded build queue, one writer/host and resource budgets
apply. Approval is invalid after input/policy changes. A Rust coordinator accepts
only generated, reviewed targets and fixed operations, not client-supplied Nix
expressions, filesystem paths, executable names, flags, substituters or credentials.
Build-time evaluation can fetch/execute build inputs: pin and review the full
input graph and use the existing daemon/sandbox and bounded launcher.

A build output, lock graph and config hash form one immutable receipt. Following
restart, inspect an existing build/result; do not replay an unknown mutation.
Build failure leaves the active generation untouched. The present host API only
returns configuration proposals and lacks this build capability: add a separately
reviewed interface rather than pretending existing grants permit it.

The UI button is **Validate & build**. The next view displays effects, artifact,
target and a separately authenticated operator procedure. Do not add agent-driven
activation, boot switching, password handling or unattended sudo. A future human-
operated activation button requires its own narrowly authorized host design and
cannot be used by coding agents under current workstation policy.

### UI surfaces

- Setup: private access, baseline status, missing dependencies and safe defaults.
- System: guided category forms, prompt-to-draft, readable diff, validation/build
  progress, rejected fields and recovery; CLI mirrors the same workflow.
- Agents: approved templates, resource/budget estimates, admitted/stopped/starting/
  ready/failed states, explicit start/stop, Durable sessions and pending approvals.
- Board: canonical issues/research runs, claims, progress, bound reports and sources.
- Monitoring: host overview, per-container/cgroup charts, service health, data
  freshness, network volume and separate policy/tool event timeline.
- Connections/security: actual grants, revocation, evidence-linked findings and
  pending human-reviewed optimization proposals, never a fictional safety score.

Fast startup is for already declared, built, reviewed stopped instances or
predeclared slots. A new image/instance/policy follows config -> build -> operator
activation. Do not conceal a mutable second instance registry behind "Create".

## Durable, application and board state

| State | Authoritative owner | Restart/restore rule |
| --- | --- | --- |
| Desired system | Approved TOML in consumer | Immutable revision; compare with observed generation. |
| Proposals/build receipts | Rust application/build service | Durable approval and target identity; uncertainty is visible. |
| Issues, research assignments, report acceptance | Independent board service | One writer; UI is a projection, not a second board DB. |
| Conversations/checkpoints/tool task states | Per-instance Pi Durable SQLite | One writer, exact runtime/schema, stable operation IDs, no unsafe replay. |
| Native run/report correlation | Thin Rust execution bridge | Map board issue/run + revision to instance/conversation/task and report hash. |
| Budgets, worker claims and approvals | Existing host services | Preserve reservations/receipts; never reset on restart/restore. |
| Host telemetry | Hostpulse archive | Bounded retention, explicit revision/gaps/consent, no authority to manage. |
| Security findings | Separate analyst state | Evidence references/proposals only; cannot rewrite authoritative events. |

For this integration the independent board is canonical. Existing UI HQ task
storage must not become a second writer for linked board tasks. Preserve existing
standalone deployments until an explicitly reviewed authority migration; do not
silently import or merge their tasks. Add an explicit issue-to-research-run mapping.
Claim with revision/CAS, persist stable execution intent, and use report-hash-bound
completion before marking the linked issue Done. Current generic board PATCH can
mark research complete without a bound report; close that policy gap first.

Research connections own transient jobs and evidence; disconnect cancels jobs.
Persist permitted final report/source provenance before finish/cancel removes
job-only evidence, respecting provider storage rights. Durable resumes safe local
work, not lost Research calls. Interrupted model sends and worker dispatches also
remain unknown/aborted under Tentaflake's no-replay policy. Prove the pinned
upstream exposes sufficient recovery control before admitting Pi; otherwise keep
the scaffold and report an upstream blocker. No home-grown replacement model loop.

## Hostpulse and live observability

Do not call the untested collector "working" because it builds. Qualify it with
synthetic sources, then an isolated NixOS runtime, then operator-activated target.
Keep it independently usable and headless on the VPS; do not install its desktop
Tauri shell merely to serve browser charts.

1. Add an optional dedicated system-service mode with explicit source consent,
   identity, persistent/runtime directories, limits and restart policy. Existing
   user-session mode remains valid. No implicit lingering or root collector.
2. Its same-UID private socket cannot simply be opened to the UI UID. Put a
   bounded read-only projection under the collector's identity, authorizing the
   exact UI peer and excluding archive mutation/full-content capture. Preserve
   Hostpulse's raw protocol boundary; no broad shared group or host-control grant.
3. Map declared instance -> exact systemd unit/cgroup -> observed workload using
   trusted metadata. Test renamed/missing/reused identities and partial coverage;
   cgroup counters are not proof of a discovered container or individual session.
4. Add bounded aggregate interface byte/rate observations as an independent
   Hostpulse feature. First/reset/denied counters are unknown, not zero.
5. Export policy-admission and tool-execution metadata from their enforcing
   services, not packet payload capture: actor, correlation ID, operation/tool,
   decision, policy revision, timing, byte/count/cost summary and outcome.
   DNS/domain/path/body details need explicit minimization and disclosure policy.

The UI reads timestamped snapshots and bounded changes, resnapshots on gaps and
applies an injected-clock-testable freshness deadline (default proposal: three
source intervals, minimum ten seconds). Disabled, missing, denied, no-sample,
partial, stale and disconnected are distinct. Never render missing as 0 or healthy.
Network counters show volume; policy events show admitted/denied actions; neither
establishes complete packet visibility or prevents exfiltration to an allowed
service. Correlation must carry provenance, not name-based guesses.

Keep full native agent capture and journal bodies off by default in the integrated
profile. Archive is plaintext and sensitive; encrypt selected backups, bound
retention/cardinality and authorize any detailed log view separately. Avoid
running both the existing optional Prometheus stack and Hostpulse by default;
choose ownership explicitly instead of duplicating collectors/storage.

## Enterprise MCP and advisory security analyst

Public-web Research remains separate from authenticated Drive/Gmail/HubSpot.
Add an optional Rust connector gateway with per-tenant/account identity, reviewed
server catalogue, pinned client/server protocol, bounded methods/resources,
runtime credentials, egress, rate/cost ceilings, audit and revocation. Arbitrary
remote MCP endpoints or unreviewed server packages are not a configuration feature.
Start with read-only grants; sending mail, sharing files and mutating CRM records
require separately approved digest-bound actions. No authentication/login or
scope widening is implied by this plan. Synthetic accounts first; actual account
consent is operator-owned. See the [security requirements](security-threat-model-plan.md).

One optional Pi Durable analyst per host, using its own capped model, can consume
redacted monitoring/policy summaries and propose findings, tuning and code changes.
It does not own the container engine, configuration approval, host-control socket,
raw archives, enterprise credentials or arbitrary tool execution. Container
management is deterministic policy plus explicit operator actions; the analyst
may recommend those actions. Prompts/logs/tool results are hostile input even to
this analyst. Detection is fallible and cannot certify absence of injection.

Any efficiency/cost improvement is a versioned proposal with evidence, expected
impact, a reproducible test and rollback. Offline fixtures are routine; real
model benchmarks/canaries require explicit budget and authorization. Never
self-modify production policy, increase grants or trade away security controls
for a cheaper token count.

## Rollout and non-goals

Keep consumer identity/hardware/secrets in `nixos-host`; add reviewed pinned
Tentaflake/UI/Hostpulse/board inputs without replacing unrelated host work. Match
the Research fork/client/protocol already pinned by Tentaflake: upstream Secure
Research Tool v2/seven tools is not interchangeable with this fork's v1/five.
Management Tailscale is not proof of Research VPN egress.

Prepare consistent SQLite backups for each writer and a reconciliation checkpoint
across board/runtime/worker/budget state. Clean stop or supported SQLite backup
must include WAL semantics. Restore drills preserve IDs, report links and replay
barriers; code rollback alone does not reverse schema changes. Never roll back
spend to create new budget or replay uncertain operations after lost receipts.

No destructive VPS reinstall, root conversion, private account enrollment,
public Funnel, automatic infrastructure mutation or guaranteed autonomous
security is part of this plan. Build, synthetic VM, live provider and target-host
acceptance are different gates. Current success is a reviewed plan, not a deployed
enterprise-ready platform.
