# Tentaflake product roadmap

Planning baseline: 2026-10-05. Status: planned, not an implementation or release
announcement. Stable M01-M10 IDs map to the GitHub milestones below.
Issue tracking does not establish implemented, verified or released support.

Requirement: deliver a common isolated environment for interchangeable agents,
with simple agent onboarding on installed hosts. General-purpose AI employees,
company workflows and business integrations build on that runtime foundation.

This document owns product direction and proposed acceptance criteria. The
[README](../README.md), [adapter guide](agent-adapters.md) and operational guides
remain authoritative for what actually works today. Proposed option names,
commands and interfaces below are not installed APIs.

## Delivery checkpoint

Active work: #115 is submitted as [PR #139](https://github.com/timfewi/tentaflake/pull/139),
commit `2567d81407828f7692ee9e837ad30a4ac3b26051`. Its CI is pending; DCO and
source scan passed. #135 is being implemented on `feat/installed-agent-onboarding`.
Delivery order: #135 installed-host validation/import, then #116 capability admission and
#136 actual coding-agent acceptance. M03-M10 and remaining P1/P2 issues retain
all their acceptance requirements. No issue is complete from planning alone.

The #115 implementation keeps legacy state/APIs, shares final image containment,
adds generic non-secret definitions and configuration-free catalog discovery.
Owning files: `adapters/catalog.json`, `adapters/generic.nix`,
`lib/runtimeContract.nix` and the operator CLI. Verified: 44 CLI tests, runtime
contract/legacy fixtures, module/image checks, CI routing, lint and the generated
installed flake. Review reproduced a nested writable bind-source substitution;
the generic layout now uses separate sources beneath an unmounted root-owned
parent, and the shared secure policy rejects nested sources, including read-only
children. PR #139 CI/merge remains the delivery gate. Live vendor acceptance,
local VM workloads and host activation were not performed.

#135 work in progress: `lib/agentsFromData.nix` accepts an in-memory object
through the same parser as file input; `lib/agentPlan.nix` validates additions
through the actual builders; `lib/agentPlanEntry.nix` accepts structured CLI
arguments. CLI template/validate/plan/import now use bounded offline validation,
an import lock and atomic publication with observed conflict checks. Actual
installed preset/generic CLI interactions passed, including redaction, refusals,
lock admission and concurrent-edit preservation. Authoritative guides and
targeted CI routing are updated. Next: final lint/Rust/installed checks, review
and a DCO-signed PR. No #135 CLI is released or activated yet.

## Product direction

The immediate priority is an agent-agnostic isolated runtime. An operator should
be able to add an agent through a small validated definition, without modifying
Tentaflake's security code or implementing an employee-profile compiler. Hermes,
ZeroClaw, OpenCode, Codex and Pi are possible workloads, not separate security
architectures. Naming an agent here does not establish current support.

Tentaflake should let individuals and companies operate AI employees that can
take any freely described professional role: legal work, marketing, research,
administration, customer service, purchasing, software development, or a custom
combination. There is no fixed profession enum or profession-based product gate.
Templates are optional and editable. Coding is one activity, not the product's
definition.

An employee has a persistent identity, personality, knowledge, assignments and
delegated authority. The same employee can change roles and use different tools
without becoming a separate user-facing employee for every activity. Human
employees and AI employees can participate in the same organizational workflows.

Tentaflake remains a secure core with optional organization, integration and
management modules. First deliver the common runtime, installed-host onboarding
and actual coding-agent acceptance. Employee-profile compilation, organization
and the company UI follow; they are not prerequisites for running an agent.

### Agreed operating model

- One independently deployed installation per company, initially on one host.
  Multiple human users, teams and AI employees are supported by the target
  organization layer. Individual operators remain a valid smaller deployment.
- One employee identity can use several isolated work contexts. Isolation is a
  technical boundary, not merely another chat in the same privileged process.
- Employees act autonomously inside standing grants. Routine model calls,
  permitted research, local execution, role changes and authorized business
  actions do not ask for approval every time.
- Human approval concerns additional authority or a specific otherwise denied
  disclosure. Agent-generated instructions and incoming messages cannot grant it.
- Company administrators manage organization, employee profiles and connections
  through the UI. Nix sets technical authority ceilings and deployment policy.
- Company identities, real credentials, private SOUL files and deployment data
  stay out of the generic template repository and the Nix store.

### Inspiration and evidence limits

[OpenAPPA](https://github.com/archestra-ai/OpenAPPA) motivates deterministic
information-flow decisions and policy replay. The selected approach is a small
Tentaflake policy layer, not a full reimplementation or an assumed integration
of OpenAPPA. The comparison read its README, not a complete security audit.

[Paperclip](https://github.com/paperclipai/paperclip) motivated discussion of
organization and coordination. Its repository fetch did not succeed during the
comparison, so this roadmap makes no verified feature-parity claims. Its product
requirements come from the decisions recorded here.

[NVIDIA OpenShell](https://docs.nvidia.com/openshell/about/architecture) informs
the separation between an ordinary agent workload and externally enforced
isolation, policy and credentials. The comparison covered current 0.1.x docs
and selected source, not runtime testing or a complete security audit. Reusing
OpenShell is optional and requires a bounded compatibility decision; NixOS and
Tentaflake's gVisor, broker and Research boundaries are not assumed compatible.

## Starting point

| Existing foundation | Remaining product work |
|---|---|
| NixOS host, systemd supervision, gVisor capsules and runtime inventory | Organization and employee lifecycle above individual runtime instances |
| Hermes/ZeroClaw adapters and native configuration generation | Shared runtime definition, small agent presets and installed-host onboarding; later employee-profile compilation |
| OpenClaw stopped scaffold | Reviewed artifact and actual model, Research, execution and lifecycle acceptance |
| Broker-held provider credentials, route/model controls and budgets | Common data/action policy across all capabilities |
| Isolated Research service and disposable offline workers | Verified mediation of actual vendor tools and controlled business connectors |
| Worker requests, private approvals and bounded result export | Atomic claims, crash recovery and queue capacity |
| Scoped Git auto-push | Immutable publication requests subject to the shared policy |
| Restart backoff, quotas, encrypted backup and optional observability | Application readiness, hangs, new-state restore and tested notifications |
| Operator CLI | Multi-user management, OIDC, workflow service and company UI |

The current adapter contract provides format and generation hooks; it does not
translate a common employee profile into semantically equivalent native options.
Current `seedDir` imports do not overwrite existing persona files. A generated
runtime configuration may be read-only. OpenClaw is not operational support.
See [agent management](02-agent-tips.md) and [adapters](agent-adapters.md).

## Architecture decisions

### Common isolated runtime and small agent presets

One versioned runtime definition describes a reviewed digest-pinned image,
argument vector, non-root identity, workspace/state, resource bounds, lifecycle
mode and explicitly requested capabilities. Secrets use runtime credential
channels, never the definition or Nix store. The common runtime owns isolation,
inventory, supervision and diagnostics independently of the agent's name.

An installed host supports selecting a supplied preset or importing a small
local definition, validating it, inspecting the planned changes and applying it
through the existing operator-controlled configuration workflow. An arbitrary
supported image and start command must not require a new upstream adapter.
Validation precedes activation; failed validation preserves the active revision.
Do not promise activation without a host configuration update or install tools
and download unreviewed images as a side effect of importing a definition.

Agent presets add only verified native settings, paths, model/Research hooks and
terminal requirements. The simplest workload needs an image and command plus
the common runtime defaults. Configuration, authentication and session semantics
may still differ; unsupported integrations remain visibly unavailable. Profiles
cannot add arbitrary host mounts, sockets, network routes or capabilities.

Native shell, file, Git, compiler and test tools run inside the isolated
workspace, within standing grants. Disposable workers are a separate capability
for tasks requiring another bounded execution environment. They are not the
universal transport for every in-sandbox shell command. This is a proposed mode,
not a relaxation of today's secure startup assertions or vendor support claims.

Keep a mandatory common containment/resource/workspace baseline. Admission
checks each requested capability against its exact controls: model access needs
the LLM broker; Research needs its isolated relay; separate worker execution
needs its worker policy. Absent or policy-denied capabilities expose no matching
tools, credentials or mounts. Unsupported or ambiguous requests fail closed.
Retain the single Research transport and prohibit direct/provider-hosted web
tools. Check real boundary readiness before launch and distinguish configured,
ready, stopped, failed and unknown evidence.

Agent selection is independent of the isolation backend. Reuse the existing
OCI/gVisor foundation first; qualify any alternative backend separately. Do not
build several backends or replace enforcement merely to add an agent. `strict`
remains unavailable until its separate kernel boundary is accepted.

One versioned catalog owns preset identity, artifact, capabilities, constraints
and evidence status. CLI discovery, documentation tables and website summaries
derive these facts from that catalog. Authoritative guides remain in this public
repository; publication follows the separate release-pin workflow. Unverified
presets and stopped scaffolds cannot appear as operational support.

### Independent integration boundaries

| Family | Responsibility |
|---|---|
| Isolation backend | Enforce the common process, filesystem, network and resource boundary; independent of agent selection |
| Agent preset / native adapter | Image and command, with optional native configuration, tool discovery and session hooks; no separate security implementation |
| Model-provider adapter | Map supported model protocols behind the existing broker while preserving credentials, budgets and streaming limits |
| Application connector | Expose resource-scoped business operations and events independently of the chosen agent runtime |

Adding a CRM connector must not require implementing it again for every runtime.
An agent's access to a program is separate from its conversational persona.

### Optional employee configuration compiler

After the common runtime is accepted, the user edits employee concepts through
UI or CLI: identity, persona/SOUL, roles, instructions, skills, model preferences,
memory, knowledge and tools.
The flow is:

```text
UI / CLI / supported import
          -> versioned Tentaflake employee profile
          -> capability and policy validation
          -> versioned runtime adapter
          -> validated native files and settings
          -> activation at the supported lifecycle boundary
```

The shared layer is a configuration compiler, not just a YAML/TOML/JSON parser.
Hermes currently generates YAML; ZeroClaw generates TOML. OpenClaw's exact
native parsing and file requirements must be accepted against its selected
version before activation; its current JSON-marked scaffold is not that proof.

- Keep a canonical, versioned profile. Generated native files are projections,
  not a competing source of truth. Compilation is deterministic for the same
  profile, adapter version and host policy.
- Adapters declare supported capabilities, constraints, file locations and
  whether changes need a new session, reload or restart. Equivalent labels in
  two products do not prove equivalent behavior.
- Keep the common input structure stable. Show unsupported features and the
  reason; retain drafts, but reject activation of unmet requirements. Do not
  silently drop fields, truncate persona text or approximate memory semantics.
- Offer validated runtime-specific settings in an advanced view. They cannot
  replace broker routes, expose credentials or override host security controls.
- Import known native settings with provenance. Preserve unmapped settings in
  the import report/draft for review; do not silently activate unknown options.
- Validate complete output before activation, preserve the previous revision
  and detect conflicts with out-of-band edits. Publish a consistent revision,
  not a partially updated collection of files.
- Preserve existing builder APIs and state ownership. Profile migration is
  explicit. Changing runtimes does not imply lossless migration of conversations,
  memory or unsupported native features.

### Employee profile and personality editing

The UI location is **Employees -> employee -> Personality & knowledge**, with
sections for identity/SOUL, roles and working instructions, skills, knowledge and
memory, programs and permissions, native runtime settings, and revision history.

Use free-text/Markdown editing, native file previews, diffs, import/export and
rollback. Templates remain optional and customizable. Store company profiles
in the private organization state and project only the permitted revision into
each context. In Hermes, SOUL is associated with `HERMES_HOME`; other mappings
must be verified rather than inferred from matching filenames.

Each employee selects one durable-persona editing mode:

1. Human-managed.
2. Agent proposes changes for review (initial default).
3. Agent may update its persona autonomously, with revision history.

Memory writing is separately configurable and defaults to automatic within its
authorized data scope. Persona self-edits use the profile service, never grant
technical authority and cannot lower data classification. An adapter exposes
this mode only after its update path is verified. Skills containing executable
tools remain subject to the same installation and capability controls.

Drafts can be saved without activation. Activating a profile creates a revision;
running tasks keep their pinned revision, and new tasks/sessions use the new
one. Show pending application and any required restart. Rollback creates an
auditable revision rather than erasing history. Imports preserve existing
persona and memory until the user explicitly adopts the managed profile.

### Policy, context and memory boundaries

Effective permission is the intersection of host ceilings, company policy,
employee grants, task scope, context rights and provider permissions. A role or
organizational reporting line is not an authorization grant.

The first policy engine is a pure Rust library returning `allow`, `deny` or
`approval-required` with stable reason codes. Host enforcement points provide
identity and normalized facts. The engine has no credentials or tool execution.
Replay is offline and has no external effects.

Initial data classes are `public`, `internal` and `confidential`, combined with
team and resource permissions. Operators assign classification; this is not an
automatic secret detector or a complete dynamic information-flow system.

- Treat all data accessible to a context conservatively at its assigned level.
  Contexts cannot label themselves public or obtain credentials for another one.
- Public web research runs in an appropriately restricted public context.
  A confidential context uses only model/application destinations explicitly
  authorized for its data; its unrestricted Research capability is absent.
- Preserve the existing isolated Research transport. Business connectors are
  separate typed capabilities, not a replacement general web-fetch route.
- Partition memory and profile fragments by permissions. Summaries, transformed
  data and artifacts retain at least their source classification.
- Automatically allow declared transfers to equally or more restricted contexts.
  Lower-class disclosure requires authorization for the exact frozen output and
  destination. A new role, session or renamed instance does not declassify state.
- Keep operator instructions separate from agent-written memory and untrusted
  external content. No shared writable workspace spans security contexts.

Standing grants allow routine actions without prompts. One-time approvals bind
the actor, immutable request/content, destination and policy revision; expire
after 15 minutes; and are consumed once. Relevant changes invalidate them.
Unknown external outcomes are reconciled where possible and never blindly
replayed. Waiting for approval affects the relevant workflow branch.

### Company organization and durable state

Model company, human users, AI employees, teams, roles, projects and processes.
Support multiple team memberships, reporting relationships, responsibility and
human takeover without prescribing a particular company's structure.

Initial human roles are company administrator, team manager, member and auditor.
Profile editing, resource access and action approval require explicit delegated
permissions; team membership alone does not expose all company data. Use OIDC
for login with explicit identity/group mapping and retain local operator CLI
recovery. The company UI does not receive a general host shell or OCI socket.

The optional organization service uses PostgreSQL for identities, profile
revisions, grants, tasks, connections and budget reservations. Large documents
and artifacts use separate protected storage. Keep full prompts, messages and
tokens out of operational audit logs. Record actors, revisions, decisions and
bounded identifiers; protect content digests and metadata according to scope.

Hierarchy budgets are company -> team -> employee -> execution. Allocate bounded
shares and reserve before dispatch so concurrent contexts cannot overspend.
Distinguish reservations and estimates from confirmed provider usage/cost.

### Business connector platform

Each versioned connector operation declares input/output schemas, provider
scopes, resource selection, data classification, destinations, rate/size/time
limits, effect type, idempotency and reconciliation behavior. Reading, creating
drafts, publishing and deleting are separate capabilities.

Connections distinguish personal, bot/service and explicitly delegated access.
OAuth lifecycle includes state/PKCE validation, scoped consent, refresh and
revocation. Store dynamic tokens encrypted with a runtime-supplied master key;
do not give provider tokens to employee processes. Fixed service credentials
continue to use host runtime credential channels.

Native adapters cover common services. OpenAPI imports produce disabled drafts
whose operations, effects, scopes and targets require review. MCP extensions
must be pinned and reviewed; tool descriptions are untrusted metadata. Isolate
extension processes and restrict their credentials and egress per connector.
Neither path grants a general authenticated HTTP or arbitrary tool proxy.

Incoming events are authenticated, size-bounded, deduplicated and durably queued.
Prefer supported outbound subscriptions/polling. Public webhooks require an
explicit separate ingress with signature/replay checks, not a public agent API.
Revocation blocks new access; already-dispatched effects must be reported
honestly rather than claimed to have been undone.

Per-operation support states are experimental, verified and deprecated. A
connector name does not imply complete coverage of the provider's product.
Verify current primary API documentation, scopes and provider constraints before
implementing each connector; mock tests alone do not confer verified status.

## Milestone map

All items start as **planned**. GitHub milestone descriptions own the current
planning status (`Status: planned` or `Status: in-progress`). A closed milestone
means completed tracking work, not automatically verified or released support.
Use `Status: verified` or `Status: released` only with an `Evidence:` URL;
released status requires a published release link. Dependencies describe delivery gates, not dates
or permission to run every check at once. Early runtime acceptance and queue
reliability belong to the security work, not a distant polishing phase.

| ID | Milestone | Depends on | User-visible outcome |
|---|---|---|---|
| M01 | Common isolated runtime and simple onboarding | Existing adapter foundation | Add an agent on an installed host without changing security code |
| M02 | Coding-agent acceptance and policy enforcement | M01 contracts and onboarding | Two different agents perform repository work inside the same enforced boundary |
| M03 | Company identity and employee editor | M01, M02 | Manage people, teams, personas and permissions |
| M04 | Employee contexts, memory and runtime acceptance | M02, M03 | One employee can safely perform many roles |
| M05 | Connections, actions and event foundation | M02, M03, M04 | Controlled business access and external effects |
| M06 | Business integration catalog | M05; delivered in waves | Useful coverage of everyday company programs |
| M07 | Tasks, processes and collaboration | M04, M05; a verified event connector | Autonomous multi-step work with human participation |
| M08 | Continuous operation and recovery | M04, M05, M07 | Observable and recoverable unattended operation |
| M09 | Complete company console | M03, M05, M07, M08; released M06 connectors | One place to manage employees and business work |
| M10 | Release, migration and operational acceptance | M01-M09 for the selected release scope | Clearly documented, tested support and upgrades |

### M01 — Common isolated runtime and simple onboarding

Deliver the common runtime definition, one shared builder, capability catalog
and installed-host select/import/validate/preview workflow. Keep native adapters
small; migrate known consumers in the same change rather than retaining a second
security implementation. Preserve identified legacy builder/state consumers.
Choose one coding-agent reference and record whether OpenShell reuse is useful;
using OpenShell is not a delivery dependency.

Acceptance:

- An installed-host fixture adds a generic image/command workload and a supplied
  preset without editing upstream modules or implementing a new Rust adapter.
- Unknown versions/options, unpinned artifacts, forbidden mounts and unsupported
  capabilities fail with actionable reasons before changing active state.
- CLI discovery and generated support facts agree with the versioned catalog;
  declaration, fixture evidence and actual vendor acceptance remain distinct.
- Existing consumers and state ownership remain compatible; mixed-version and
  conflicting edits are detected. Credentials never enter generated store paths.
- No company service, OIDC, employee persona or new backend is required. Real
  isolation and vendor behavior are accepted separately in M02.

### M02 — Coding-agent acceptance and policy enforcement

Implement capability-specific admission for the proposed runtime mode without
weakening existing containment, broker, credential, quota or Research controls.
Accept one pinned coding agent end to end, then a second independently configured
agent through the same runtime. OpenCode is the first candidate; choose the
second from Codex or Pi after verifying artifact, model and session compatibility.
Unsupported candidates remain unaccepted instead of weakening the boundary.

Exercise interactive attach, native file/shell/Git/test tools, bounded writable
state, model calls, cancellation, restart and explicit stop. Deny host and
cross-agent files/sockets and undeclared network destinations. Research, when
requested, uses only the exact relay; a denied capability has no exposed tool or
mount. Additional worker access is separately accepted and unavailable without
its exact policy. Instructions alone are never enforcement.

Implement the common policy library, decision records and offline replay at the
LLM, Research and optional worker boundaries. Retain actual Hermes/ZeroClaw
acceptance work alongside the coding references. Verify native execution inside
the capsule separately from any advertised automatic routing to workers; neither
an enabled worker nor a synthetic probe proves vendor tool behavior.

Add atomic worker claims, one active drain/worker execution per configured queue,
bounded inbox/pending state and crash recovery. Distinguish not dispatched,
finished and unknown outcomes. Keep approvals private and tied to immutable
requests. Migrate pending state explicitly and do not trust old approvals by
default.

Acceptance:

- Forged identities, labels, roles and tool paths cannot increase authority.
- Allowed jobs execute without per-call confirmation; prohibited effects do not.
- Duplicate IDs, oversized/full queues, disk failure and crash points preserve
  boundedness and do not cause unreviewed replay.
- Existing broker streaming, cancellation, credential and Research boundaries
  remain intact. Policy/audit failure prevents new external dispatch.
- Both coding references edit a repository and run its focused checks through
  their native tools. Adding the second changes only its definition/native hooks,
  not common security code. Record real application and boundary evidence.
- Management-provider and optional worker work remains tracked here but does not
  delay the first coding scenario unless that scenario requests those capabilities.

### M03 — Company identity and employee editor

Deliver the versioned employee-profile compiler, capability validation,
import/diff/export and advanced native settings described above. Native parsing
and meaningful behavior need pinned-runtime evidence; unsupported fields block
activation rather than silently dropping or approximating them. Preserve
unmapped imported data and existing persona/memory until explicitly adopted.

Add the private organization service, PostgreSQL state, OIDC and delegated
administration. Provide the initial employee/profile editor described above,
including unrestricted role text, native previews, self-edit modes, history and
rollback. This is the early UI needed for the product, not the complete console.

Acceptance:

- Different users can see and edit only their assigned employees/resources.
- Seeded persona and memory import safely; a changed profile is demonstrably
  loaded by its pinned runtime at the advertised lifecycle boundary.
- Running tasks retain their revision. Concurrent edits, invalid native options
  and unsupported self-edit modes cannot silently overwrite active state.
- Profile/role edits cannot bypass Nix ceilings or gain host authority.

### M04 — Employee contexts, memory and runtime acceptance

Create host-managed context identities, per-context state and permission-aware
knowledge access under one employee identity. A structured store with search is
the initial memory implementation; a vector database is not a prerequisite.
Transfer frozen results through policy, with provenance and protected labels.

Extend the M02 runtime evidence with employee context/persona/memory behavior.
For OpenClaw, select a reviewed image and verify native configuration, persona,
model streaming, Research discovery, any advertised worker mediation, lifecycle
and persistence before changing its support status. Do not omit OpenClaw merely
because its current adapter is a scaffold.

Acceptance:

- One employee performs non-coding and coding tasks using different roles.
- Public contexts cannot read confidential memory, profile fragments or another
  context's credentials. Summarization and new sessions do not remove labels.
- Configured low-to-high transfers proceed automatically; unauthorized reverse
  transfers fail and authorized releases bind exact content and destination.
- A mock runtime cannot establish vendor acceptance. Unsupported adapters remain
  visibly unavailable without weakening the secure profile.

### M05 — Connections, actions and event foundation

Build the connector SDK, scoped connection management, OAuth/token lifecycle,
operation admission, event ingestion and private connection UI. Supply local
test connectors for messaging and record changes, covering non-coding work.

Deliver Git as the first real external-write reference: capture a bounded,
immutable repository snapshot, include transferred history in disclosure review,
and bind exact commit, allowed repository/branch and expected remote state.
Do not execute agent-controlled hooks, configuration or credential helpers in
the trusted publisher. Standing grants permit matching pushes automatically;
force-push and ref deletion are excluded initially. Reconcile uncertain outcomes
without blind retries. Replace legacy auto-push in the new policy mode.

Acceptance:

- Agents never receive provider tokens; revoked or wrong-resource connections
  cannot dispatch new work. Token refresh failure remains observable.
- Post-approval mutation, destination changes and malicious Git metadata cannot
  alter an authorized effect. No parallel legacy publication path bypasses it.
- Duplicate/forged events do not create duplicate logical work or new authority.
- Tests cover rate limits, timeouts, partial responses and uncertain write results.

### M06 — Business integration catalog

Deliver these waves through the same contracts. They are product targets, not
verified APIs or promises of full service coverage. Track each connector and
operation separately; a release can name its accepted subset without claiming
the rest of the catalog.

| Wave ID | Area | Planned native coverage / extension target |
|---|---|---|
| M06-A | Communication and scheduling | Gmail, Outlook, Slack, Teams, Google Calendar, Microsoft calendars |
| M06-B | Documents and knowledge | Google Drive, OneDrive/SharePoint, Notion, Confluence |
| M06-C | Work management and development | Jira, Linear, GitHub, GitLab |
| M06-D | Customers and support | HubSpot, Salesforce, Zendesk |
| M06-E | Additional business systems | ERP, accounting, shops and internal APIs through reviewed SDK/OpenAPI/MCP extensions |

Acceptance per operation includes schema validation, scoped resources, pagination,
rate-limit handling, authentication/refresh/revocation and effect recovery. A
verified release requires an explicitly selected provider test-account exercise
as well as local contract tests. New connectors may not introduce a general web
transport or arbitrary credential-bearing endpoint access.

### M07 — Tasks, processes and collaboration

Manage assignments, goals, owners, dependencies, roles, data scopes, budgets and
result references. States distinguish ready, running, awaiting approval,
succeeded, failed, cancelled and outcome unknown. Support manual assignments,
declarative schedules and authorized business events.

Employees propose decomposition, role changes and delegation; the host checks
them. Humans can take over, correct, cancel and reassign. Initially one main task
runs per employee, with multiple employees operating concurrently within host
and budget limits. Define bounded schedule catch-up rather than an unbounded
backlog after an outage. Provider/adapter cancellation does not promise reversal
of already dispatched actions.

Acceptance:

- A mixed business workflow researches, processes information and performs an
  allowed connector operation under one employee identity.
- Roles can be arbitrary text; task execution is not tied to a coding profession.
- Allowed branches continue autonomously; approval waits are localized.
- Task/event deduplication, dependency failures and budget reservations survive
  restart. Delegation never grants access to another employee's private state.

### M08 — Continuous operation and recovery

Extend existing supervision with non-billable adapter readiness/liveness probes,
bounded hang recovery and cooldown. Respect explicit operator stops. Expose
categorical health, queue, policy, connection, Research, budget and restart
metrics without sensitive content; test a generic configured notification route.

Back up and restore consistent organization/employee/queue/connection state as
well as managed workspace mounts. Preserve classification and policy revisions;
restored one-time approvals are not automatically active. Reconcile in-flight
external effects, and handle provider-side revocation after restore.

Acceptance includes dependency loss, repeated transient failures, hanging
processes, reboot, quota exhaustion and fresh-host restore. Reuse existing
recovery/backup foundations rather than replacing them unnecessarily.

### M09 — Complete company console

Combine organization, employees, personalities, tasks, processes, integrations,
approvals, budgets and operational status in one private multi-user application.
Use a Rust backend with server-generated pages and minimal JavaScript as the
initial implementation. Use private HTTPS/OIDC, protected sessions and CSRF
checks; keep public webhook ingress separate.

Permissions apply equally to CLI/API/UI. Provide scoped task actions, employee
start/stop, connection management and explicit approval review. Permanent host
ceiling changes and NixOS activation remain outside the company UI. Render
agent-provided content as untrusted data, not executable embedded HTML.

Acceptance includes accessible keyboard operation, login/session behavior,
unauthorized object access, malicious content, long-running task states and
honest unsupported/stale/unknown indicators. The console must make the difference
between employee roles, runtime capabilities and technical rights understandable.

### M10 — Release, migration and operational acceptance

Publish the supported matrix and tested configuration for the chosen release.
Exercise installation, upgrades, state/profile migration and restore together.
Preserve legacy builders; require explicit adoption of the new employee/company
mode rather than claiming existing installations already implement it.

Acceptance requires verified reference scenarios for an individual employee and
a company workflow, corresponding documentation and examples, and recorded
residual limitations. Distinguish source checks, simulated providers, real pinned
runtimes, provider test accounts and production-host evidence. Do not assign a
calendar deadline or version promise solely from this roadmap.

## Proposed public surfaces and implementation discipline

These names express intended interface boundaries, not existing options:

| Proposed surface | Responsibility |
|---|---|
| Common runtime definition / preset catalog | Image, command, workspace, resources, capability requests and small native hooks; final API chosen in M01 |
| `tentaflake.policy` | Host ceilings, classification and allowed capability boundaries |
| `tentaflake.organization` | Organization service, storage and identity provider |
| `tentaflake.connectors` | Approved implementations, runtime credentials and network boundaries |
| `tentaflake.orchestration` | Task/process service and execution limits |
| `tentaflake.console` | Private management application |

Profiles, teams, employees, connections and delegated rights are managed through
the authorized service API. CLI groups cover `policy check/explain/replay`,
`profiles import/export/diff/rollback`, `approvals list/show/approve/deny`, and
`tasks create/list/show/cancel/retry`. Version machine-readable contracts and
retain existing inventory compatibility. Import/export respects content rights
and excludes credentials.

Share Rust policy/contract libraries across the relevant binaries. Explicitly
include their source in Nix package filesets and retain the
[Rust packaging cache checks](17-builds.md). Schema migrations preserve state,
require backups and revalidate authorizations; they do not silently reformat
workspaces or grant broader access.

Each implementation change records requirement -> artifact -> check -> docs.
Start with `just fast` and focused policy/adapter tests. Select affected VM suites
explicitly for runtime/security evidence; do not run a full build, VM workload,
provider-account test or host activation merely because it appears here.

Cross-cutting negative tests cover forged identities and labels, role changes,
secret-free configuration generation, profile conflicts, malicious external
instructions, denied resources, expired/revoked credentials, approval mutation,
duplicate events, queue/disk exhaustion, crashes and uncertain external outcomes.
Actual vendor parsing/tool discovery and UI interactions need separate evidence
from generated configuration snapshots.

## GitHub tracking and public roadmap

All nine open issues (#106-#114) were reviewed on 2026-10-04 before creating
these milestones. Existing management, Research, runtime acceptance, queue,
health and alert-delivery work is retained; new issues cover the missing scope.
The 2026-10-05 priority revision moves the shared runtime and simple installed-host
onboarding into M01, coding-agent acceptance into M02, and employee-profile
compilation into M03. Hermes/ZeroClaw runtime acceptance moves from M04 to M02.
M06-A through M06-E remain catalog workstreams inside M06. Milestones have no
invented due dates or version promises.

| Milestone | Work items |
|---|---|
| [M01](https://github.com/timfewi/tentaflake/milestone/1) | [#115](https://github.com/timfewi/tentaflake/issues/115), [#135](https://github.com/timfewi/tentaflake/issues/135) |
| [M02](https://github.com/timfewi/tentaflake/milestone/2) | [#106](https://github.com/timfewi/tentaflake/issues/106), [#108](https://github.com/timfewi/tentaflake/issues/108), [#110](https://github.com/timfewi/tentaflake/issues/110), [#111](https://github.com/timfewi/tentaflake/issues/111), [#113](https://github.com/timfewi/tentaflake/issues/113), [#116](https://github.com/timfewi/tentaflake/issues/116), [#136](https://github.com/timfewi/tentaflake/issues/136) |
| [M03](https://github.com/timfewi/tentaflake/milestone/3) | [#117](https://github.com/timfewi/tentaflake/issues/117) |
| [M04](https://github.com/timfewi/tentaflake/milestone/4) | [#118](https://github.com/timfewi/tentaflake/issues/118), [#119](https://github.com/timfewi/tentaflake/issues/119) |
| [M05](https://github.com/timfewi/tentaflake/milestone/5) | [#120](https://github.com/timfewi/tentaflake/issues/120), [#121](https://github.com/timfewi/tentaflake/issues/121) |
| [M06](https://github.com/timfewi/tentaflake/milestone/6) | [#122](https://github.com/timfewi/tentaflake/issues/122), [#123](https://github.com/timfewi/tentaflake/issues/123), [#124](https://github.com/timfewi/tentaflake/issues/124), [#125](https://github.com/timfewi/tentaflake/issues/125), [#126](https://github.com/timfewi/tentaflake/issues/126) |
| [M07](https://github.com/timfewi/tentaflake/milestone/7) | [#127](https://github.com/timfewi/tentaflake/issues/127) |
| [M08](https://github.com/timfewi/tentaflake/milestone/8) | [#107](https://github.com/timfewi/tentaflake/issues/107), [#109](https://github.com/timfewi/tentaflake/issues/109), [#112](https://github.com/timfewi/tentaflake/issues/112), [#114](https://github.com/timfewi/tentaflake/issues/114), [#128](https://github.com/timfewi/tentaflake/issues/128) |
| [M09](https://github.com/timfewi/tentaflake/milestone/9) | [#129](https://github.com/timfewi/tentaflake/issues/129) |
| [M10](https://github.com/timfewi/tentaflake/milestone/10) | [#130](https://github.com/timfewi/tentaflake/issues/130) |

Product direction, dependencies and acceptance criteria are owned by this
document. GitHub owns work-item state and the explicit milestone planning
status. Update this document when scope changes; do not duplicate the roadmap
in milestone descriptions.

The public website may present a dated planning snapshot at /roadmap with
source-revision and milestone links. Planning can advance independently of
release-pinned operational documentation; identify it as planned work and
show the exact source revision and snapshot date. Issue closure alone does not
establish actual vendor acceptance or released support. Website publication
remains a separate maintainer action.

## Sequential delivery and autonomous work

Deliver M01's runtime contract (#115) before its installed-host onboarding (#135).
Next implement capability admission (#116) and accept M02's first coding
reference and then its second (#136), sharing the same containment.
Only then begin M03's employee compiler and organization layer. Additional
runtime presets can follow this path independently of the company feature set.

Each issue is a bounded change with requirement, artifact, proportionate checks,
updated docs and a DCO-signed PR. Review dependencies and current source before
starting the next issue; milestone order is not numerical issue order. Save a
short checkpoint in the issue/PR with evidence, remaining work and the next
dependency so another session can resume. Do not close blocked or partially
accepted work just because the implementation compiles.

An autonomous development assignment names the milestones/issues and whether
green-CI merges are authorized. Heavy VM/ISO/system builds, provider test-account
access, deployment, host activation and destructive state migration require
authorization covering those operations. Prepare the concrete change first.
Keep checks foregrounded and within the host resource budget. A chat session
does not by itself provide a durable multi-day executor.

Later evaluations include finer-grained information-flow
tracking, cross-runtime conversation migration, multi-host operation, shared
multi-company hosting, arbitrary desktop GUI automation and a separately tested
kernel boundary for `strict`. They are not initial delivery commitments. Generic
business integrations and open-ended professional roles are part of the target
product; no finite connector list claims support for every program or operation.
