# Integrated-system implementation backlog

Status: partial implementation, not an accepted deployment. The operator lifted
the earlier integration-cycle cap; all required work remains in scope. Approved
SSH access and a bounded read-only target inventory are separate from software/
runtime acceptance; no deployment or activation is implied.
Scope and ownership: [architecture](system-architecture-plan.md).
Security scenarios S01-S20: [threat model](security-threat-model-plan.md).
Execution contract: [autonomous goal](autonomous-implementation-goal.md).

## Priority, authority and work accounting

P0 blocks safe integration; P1 completes the operator product; P2 adds enterprise
connectors/advisory optimization after foundational acceptance. Implement vertical
slices, not all backend rewrites at once. Every item is open until its observable
tests and owning repository gates pass. A passing source gate is not a live test.
The first autonomous implementation run consumed 20 integration/verification
cycles, followed by the authorized continuation through global cycle 40. The
operator subsequently lifted that cycle cap. Preserve the historical accounting
and continue authorized work until the acceptance criteria are met or a specific
authorization/upstream blocker requires handoff; no arbitrary cycle stop remains.

All identified open points are required delivery work, not optional follow-ups.
P0/P1/P2 describes order, not permission to omit lower-priority packages. An
optional feature is opt-in at runtime; its implementation and applicable tests
remain in scope. The register below must be closed with evidence before full
completion, including the operator-dependent target gate.

One integration owner coordinates parallel workers on disjoint files/interfaces.
Workers do not run checks/builds/formatters mid-flight; the owner verifies after
edits land, sequentially per repository. Agree schema and protocol contracts before
delegating callers. Preserve unrelated staged Hostpulse and host-configuration
work. No blanket formatting or commits/pushes in project repositories.

## Blocking external evidence

| Blocker | Effect | Clearing action and owner |
| --- | --- | --- |
| X01: approved SSH route restored and bounded read-only inventory verified | The access blocker is cleared; this does not qualify application health, runtime admission, private UI or deployment | Keep exact target facts and commands in operator-private evidence outside Git. Complete consumer/state/resource review and runtime/target acceptance under their owning backlog items; no inferred DNS/VPN/enrollment or production changes. |
| X02: coordinated worktrees and required product inputs are not yet published as a reviewed coherent snapshot | Local gates qualify only their exact source inputs; the private host still pins older product revisions and has unrelated staged changes | Preserve the single integration owner, review all scoped diffs and missing integration evidence, and publish only with the required repository-specific authorization. Do not substitute a local ephemeral path for a reproducible production pin or reset unrelated work. |
| X03: board has no discovered root/manifest license grant | No assumption that copying, vendoring or publishing board code is authorized | Owner clarifies licensing before redistribution; use a separately configured process/API boundary, not copied components meanwhile. |
| X04: pinned Pi runtime admission not qualified | Synthetic real-SDK success, clean reopen and no-replay now pass, but do not admit a real provider, agent container or production target | Finish B06 lifecycle, Research, isolation and operator-authorized target evidence. Existing uncertain receipts stay fail-closed. |
| X05: private management and Research egress not qualified | Cannot claim network readiness from Tailnet enrollment | Operator reviews separate management policy and Research VPN/controller/lease; isolated tests then target evidence. |
| X06: real provider/enterprise credentials and budgets absent from task data | No paid model or private-account acceptance | Separate owner consent, runtime provisioning and explicit workload/account/data/action budget. No login or secret collection to make a test pass. |
| X07: activation is operator-only | No production rollout by autonomous coding agent | Deliver exact reviewed target/artifact/effects and operator commands; owner activates, then grants bounded post-activation checks. |

## Mandatory open-point closure register

No entry is closed. The earlier 20-cycle run ended with a partial/no-go handoff;
the continuation has produced additional bounded local evidence below. Passing
results apply only to their tested inputs, not later source changes or a target
deployment. Allowed states are open, in_progress, blocked, awaiting_operator and
verified; only verified means closed. Runtime requirements cannot be closed by a
prepared patch or mock. Keep raw private evidence outside Git.

| ID | Required resolution | Owning work | Current status / established subset | Remaining closure evidence and next action |
| --- | --- | --- | --- | --- |
| O01 | Approved VPS access and capacity/runtime inventory | B00, B12; X01/X02 | **in_progress**: operator and agent UID checks passed over the approved route; canonical peer/name and sole SSH-host role matched; bounded OS/capacity/root-filesystem/cgroup and selected service/PATH/state-root metadata were collected privately | Review the coherent consumer inputs, actual state/mount/resource requirements and supported runtime before closure. PATH absence is not proof that software exists nowhere; no application health or deployment is inferred. Exact commands/facts remain outside Git. |
| O02 | Qualified independent Hostpulse 24/7 service and safe UI access | B08-B09; X02 | **in_progress**: Hostpulse fast gate and headless package pass; the synthetic no-NIC workload/projection VM now passes with the hardened non-root collector, distinct reader UIDs, denied root/outsider/cross-workload reads, and collector/container stop/restart assertions. Repeated synthetic collector restarts use an explicit operator `reset-failed` action; the production 60-second/3-start limit is unchanged | Qualify safe UI HTTP/browser access, remaining lifecycle/default-data coverage and operator-authorized target observations. The VM proves only its isolated synthetic service/projection subset, not 24/7 target health or deployment. |
| O03 | Workload mappings, live charts, network counters and policy/tool events | B09-B10 | **in_progress**: the no-NIC synthetic workload VM passes distinct container/cgroup mapping, bounded CPU/memory/I/O comparisons against kernel counters, exact-UID scoped projection, stop/recreate generation, expired lease, corrupt provenance/authority and reader-revocation assertions | Complete A07-A08 chart/freshness/gap behavior through actual UI HTTP/browser execution, network-counter and policy/tool-event provenance, and credential/content canaries. The VM does not exercise UI HTTP, browser behavior or native run attribution; it is not network/packet visibility or target acceptance. |
| O04 | Real pinned Pi Durable runner with safe recovery and mediated effects | B06; X04/X02 | **in_progress**: pinned real SDK with synthetic in-process transport passes committed success, distinct sessions, clean reopen and truncated-response no-replay; all 27 existing package recovery tests pass. A numeric SQLite submission-ID mismatch was corrected for new receipts | Qualify real container shutdown, runtime admission, Research and mediated effects; old mismatched receipts are not silently rewritten. Real provider/model workloads need a separate grant. Synthetic execution is not production acceptance. |
| O05 | Report-bound Board completion and one canonical task authority | B07 | **in_progress**: Board CAS and report-bound acceptance tests pass; completed linked issues can reorder without a direct Done bypass. Exact per-issue machine grants, session-controlled provisioning and transactional Research assignment/worker authorization were added. Board fast and full gates pass locally, including 262 tests. An opt-in UI shows retained history read-only and deliberately disables Board access pending browser-origin isolation | Complete UI/Board/Durable authority migration, preserve prior task state and prove restart/cancellation. Resolve how pre-existing login links and sessions are handled when a member is later promoted; no second editable task store or live credential is implied. |
| O06 | Exact Research protocol, relay identity and management/egress boundary | B06, B10, B12; X05 | **in_progress**: the selected five-tool protocol, exact instance relay and offline fetch denial passed in a NIC-less Docker/gVisor VM for synthetic Hermes and ZeroClaw capsules and the Pi mediation client. Stock runsc denied the same host socket; Research and Pi policy and schema checks passed | Qualify remaining cross-agent/revocation and target management/egress boundaries and real vendor acceptance. The test had no provider or model calls, Tailnet or VPS access; the guest Tailscale daemon's attempted outbound connections were unreachable. |
| O07 | Board reuse/licensing and preservation of unrelated work | B00, B07, B12; X02/X03 | **blocked**: no license grant was invented; host and project worktrees retain unrelated owner changes | Owner resolves redistribution licensing. Establish exact integrated inputs/diffs and preserve unrelated staged/unstaged work before publication; a clean initial status is not proof that the final source snapshot is complete. |
| O08 | Complete schema migration, obsolete-code/docs cleanup and consistent restore | B01-B05, B11 | **blocked**: generated-flake and full evaluation pass. One earlier fast run failed a CLI diagnostics assertion without exposing the child error; later runs passed, but the original cause remains unknown. With explicit approval, the original storage runner's recursive synthetic module graph was replaced by real declared NixOS options; its 94-case Rust/Nix corpus, normalization, redaction and backup projections now pass. Several categories remain disabled-only | Complete enabled category/native-policy coverage and A01-A03/A12, and prove coordinated restore of IDs, receipts and budgets. The runner uses synthetic quota dependencies and excludes agent imports and full security policy: it establishes bounded storage-projection evidence, not full-system parity, quota runtime or restore. Ordinary Restic backup is not a consistent checkpoint. |
| O09 | Real private UI -> linked research -> accepted sources and live monitoring | B12, B15; X04/X06/X07 | **blocked**: no host output was approved, no deployment or activation occurred; successful SSH/inventory and local API tests are not A13 | First clear source/package/runner/egress blockers, then obtain scoped provider authorization and human activation. Only the actual private-route report/source/Hostpulse sequence closes A13. An operator command or localhost mock is not its outcome. |

Drive/Gmail/HubSpot adapters and the advisory analyst remain required source-level
deliverables in B13-B14, with synthetic authorization tests before any real account
use. Optional enablement does not make them removable from this goal. Actual
enterprise-account tests require their own consent; do not claim them exercised
or silently provision accounts to close the register.

For a blocked entry, continue independent authorized work, but retain the missing
evidence, exact human/upstream action and next verification step. Handing off an
operator command does not mark its outcome verified. The earlier cycle cap no
longer requires a stop; at any authorization/upstream handoff, list incomplete
entries explicitly rather than reclassifying them as out of scope.

## P0 foundation and security admission

### B00 — Baseline, evidence and isolated workload scope

Owner: integration lead; all six repositories. Dependencies: none.

- Record actual working trees and revisions without copying private config/logs.
- Clear X01 or retain it as a target-only blocker while local implementation moves.
- Inspect VPS facts read-only: OS/architecture, CPU, memory, free space/filesystem,
  supported runtime/gVisor/cgroups, relevant services/listeners and existing state
  ownership. Do not enumerate unrelated data, credentials, mail or raw transcripts.
- Record exact supported local build/VM launchers and budgets before workloads;
  no production reinstall, disk conversion or extra background models.

Acceptance: source map resolves; unrelated diffs unchanged; authorized inventory
has a sanitized facts/unknowns checklist. If unavailable, target gates remain
blocked rather than synthetic-green. Planning already established only source facts.

### B01 — Rust system schema and public contract

Owner: Tentaflake. Dependencies: B00.

- Define versioned typed `tentaflake.toml` model for every architecture category.
  Export JSON schema/types and generate CLI/API/form validation from that owner.
- Keep secret references and public desired state distinct from runtime state.
  Reject unsupported fields, non-finite values, excessive size/cardinality,
  executable interpolation, arbitrary module origins and policy downgrades.
- Define canonical semantic hashing, revision/CAS semantics, comment retention
  and deterministic Nix/native projections; keep a single public authority.

Acceptance: golden TOML -> typed JSON -> TOML preserves semantic values and IDs;
malformed/secret-bearing/downgrade inputs fail, unrelated fields cannot disappear;
Rust/Nix/frontend validation agree on a shared fixture corpus. S01-S03, S17.

### B02 — Explicit migration and legacy removal

Owner: Tentaflake plus UI/consumer callers. Dependencies: B01.

- Inventory all `agents.toml` parser, export, CLI, installer, fixture, docs and
  consumer references. Migrate a selected inventory/policy into a candidate new
  source, with explicit unsupported/conflict reporting and safe backup.
- Preserve every instance identity, state/workspace/owner, startup intent,
  broker/worker policy and durable state reference. Do not evaluate untrusted old
  Nix to infer settings or replace an ambiguous inventory with an empty one.
- After validated cutover delete old production file/readers/APIs/callers; no
  compatibility mode or dual source. Keep only necessary offline migration/test
  fixtures and historical references clearly marked as such.

Acceptance: representative migrations produce equivalent inventory/security
projections; conflicting inputs preserve original bytes and fail. Installed paths,
generated consumer, CLI/UI and tests use only `tentaflake.toml` at runtime. An
explicit old-format runtime input is rejected. S02, S17-S19.

### B03 — Deterministic Nix compiler and bootstrap

Owner: Tentaflake; consumer host for deployment. Dependencies: B01-B02.

- Use standard internal `flake.nix`/module outputs and reviewed locked inputs.
  Do not concatenate arbitrary TOML/model text into Nix source.
- Update empty safe template, installer output, onboarding and instance metadata.
  Keep disk facts and unrestricted recovery/credential policy consumer-owned.
- Baseline install must bring up a private UI/CLI before an agent/model exists;
  no public listener, sample auto-start or circular runtime dependency.
- Prove Btrfs quota images can be prepared on a synthetic ext4-backed host without
  reformatting root. Real existing data mounts require an offline migration plan.

Acceptance: generator golden fixtures are deterministic; `just generated-flake`
and affected evaluations pass; malicious strings remain data; initial inventory
is empty/stopped and no provider secrets enter store outputs. Synthetic installer
VM only when explicitly authorized. S01-S03, S06, S17.

### B04 — Rust application/host API cutover and role contracts

Owner: tentaflake-ui + Tentaflake. Dependencies: B01, agreed route contracts.

- Port the owning Python bridge/host routes behind existing HTTP/SSE/SQLite golden
  fixtures. Keep static frontend, browser response shapes, request identities,
  cancellation, error/freshness and visible mock behavior.
- Separate application, host management and future build capabilities. Exact UID,
  fixed socket/target, bounded input and no generic shell/path/config execution.
- Add explicit viewer/editor/build/lifecycle/connector-approval authorization.
  Do not advertise enterprise multi-user support from the existing shared token.
- Do not open both old and new writers on one SQLite database. Migrate with
  consistent backup and a contract-tested cutover; remove replaced code/callers.

Acceptance: golden HTTP/SSE and storage migration fixtures preserve known caller
contracts; stale revision, duplicate/conflicting request, CSRF, wrong peer and
role/resource denial tests pass. UI/CLI use the same backend contract. S02-S04,
S08, S10, S12, S17-S20.

### B05 — Approved configuration and bounded build flow

Owner: Tentaflake Rust build coordinator + UI/CLI. Dependencies: B03-B04.

- Forms/CLI and natural-language helper produce drafts. Show category effects,
  exact TOML diff, validation errors and prerequisites before approval.
- Bind approval to base/config/compiler/policy/locked-input revisions. Queue and
  build only fixed generated targets with installed bounded launchers/Nix daemon.
- Persist build identities, outcomes and immutable output receipts; reject stale
  approval and untrusted executable/flag/path inputs. Resume observation, not an
  unknown mutation, after restart.
- Button label and stages distinguish Validate & build from operator activation.
  No agent-triggered activation, boot switching or unattended sudo/password flow.

Acceptance: browser and CLI synthetic tests cover draft -> approve -> validate ->
build receipt, conflict, cancellation, restart and failure with unchanged active
system; exact generated fixture evaluates/builds in an authorized local workload.
Changing any approved input rejects dispatch. S01-S04, S15-S20.

### B06 — Thin Pi Durable runner and actual boundary acceptance

Owner: Tentaflake adapter/package; UI native transport. Dependencies: B03-B04;
may investigate upstream compatibility in parallel with B01.

- Pin/review actual SDK, runtime, license, exports, lockfiles and artifact; existing
  preset version is not proof of installation. Implement wiring in TypeScript,
  surrounding orchestration in Rust, not a new model loop or scheduler fork.
- First prove controls for interrupted model generation and task recovery. No
  automatic repeated possibly-dispatched model/Research/worker operation. If
  upstream cannot enforce this, retain the stopped scaffold and record a blocker.
- One SQLite writer per admitted instance; explicit schema/locking, graceful
  close, bounded concurrency and protected private state. Tool implementation
  must actually use broker, Research relay and mediated worker, including file
  mutations where required. Unrestricted native bash/Node execution is not allowed.
- Expose reviewed submit/result/status/cancel operator protocol with stable IDs;
  align UI `/v1/runs` expectations or migrate every known caller together.

Acceptance: duplicate IDs preserve results; crash before/after each dispatch
causes zero unsafe replays; full/broken storage and second writer fail safely.
Actual pinned runtime, synthetic inference and relay/worker boundaries are tested,
not a shell script impersonating the vendor. Real model/provider runs are separate
X06 workloads. S06-S08, S15-S20.

### B07 — One Kanban authority and evidence-bound research completion

Owner: team-kanban-board + UI + Rust execution bridge. Dependencies: B04, B06;
X03 cleared before copying/redistributing board code.

- Use board service as canonical for linked issues/research runs. UI HQ tables
  cannot be a second mutable authority; existing standalone data needs explicit
  authority migration, not incidental deletion.
- Add durable issue/run association, issue CAS/scoped integration identity and
  stable claim/execution/report correlation. Preserve issue keys and database.
- Close generic-PATCH completion bypass, then bind the report/hash/source policy
  transactionally to completion. Cancellation and human review remain distinct.
- Integrate server-side through reviewed API; do not weaken frame/CSP/origin
  protections to iframe an application that rejects embedding.
- Persist only permitted evidence before Research job finish/disconnect removes
  job-only sources. A broken job is not automatically started again.

Acceptance: two claims have one winner; stale/cross-owner writes fail; forged or
missing report completion is denied; UI and board converge after SSE gaps/restart.
A mock research card moves todo -> in_progress -> review/done only after accepted
report; uncertain execution stays visible. S04-S09, S12-S14, S18-S20.

## P1 operational product and monitoring

### B08 — Qualify Hostpulse and add optional 24/7 service

Owner: Hostpulse core/module; consumer composition. Dependencies: B00;
independent from schema/compiler work except for final projection.

- Preserve staged `hostpulse-store` quota work. Run declared synthetic gates
  before claiming functionality. Add explicitly opted-in system-service mode;
  keep independent user-session mode and standalone config/protocol.
- Dedicated least-privilege identity, runtime config/consent, bounded collector
  sources/storage, real systemd/proc/cgroup visibility and recovery. No root,
  container-engine socket, broad journal group or full native capture by default.
- Qualify in isolated NixOS fixture then target after operator activation. Test
  logout/24-hour-lifetime design without requiring an actual 24-hour model run.

Acceptance: synthetic fixture verifies CPU/RAM/filesystem/cgroup/service samples
against controlled values; source errors/denials remain explicit; service survives
session logout and restarts cleanly. `hostpulse config check`, `hostpulse status`
and `hostpulse sources` succeed only in approved synthetic/target environments.
Tests include quota, DB restart and no-content defaults. S10-S12, S16-S20.

### B09 — Read-only Hostpulse bridge and live charts

Owner: Hostpulse protocol/projection + UI. Dependencies: B04, B08.

- Build an explicit read-only projection authorized for the UI peer; raw same-UID
  socket cannot simply be shared across service identities. Exclude archive admin
  operations, arbitrary SQL/paths and raw content from the public contract.
- Map exact declared instance/unit/cgroup identities with evidence; add optional
  generic workload/container metadata without coupling Hostpulse to Tentaflake.
- Show CPU, memory, swap, disk space/I/O, pressure, PIDs, limits and service states.
  Add time-series charts, source/freshness/coverage, nulls and gap resnapshot.

Acceptance: controlled metrics reach browser and CLI; wrong peer and admin calls
are denied. Inject clock past configured freshness and disconnect/revision reset:
charts become stale/no-data/partial rather than zero/healthy. Renamed or missing
cgroup is unknown, not matched heuristically. S04, S10-S12, S16-S20.

### B10 — Network volume and policy/tool timeline

Owner: Hostpulse generic network collector; Tentaflake/Research/connector event
producers; UI presentation. Dependencies: B06-B09 as appropriate.

- Add bounded interface byte/rate counters with reset/removal handling. Distinguish
  host, workload and unattributed scope; do not claim packet or domain attribution.
- Publish minimized versioned enforcement/tool events with trusted producer,
  actor/correlation, operation, policy revision, decision/outcome, time and counts.
  No prompt/body/credential dump; detailed access requires its own consent.
- UI separates aggregate volume, admitted/denied actions and execution outcomes;
  sequence gaps and missing coverage are explicit. Avoid duplicate collectors.

Acceptance: deterministic counter fixtures produce exact deltas/nulls; simulated
allowed/denied/interrupted operations appear once with correct provenance and
visible dropped-event gaps. Full-content canaries absent in routine output.
Missing events never imply no attempted traffic. S05-S08, S10-S12, S16, S20.

### B11 — Operations, state-consistent backup and docs cutover

Owner: Tentaflake plus each data owner. Dependencies: B02-B10.

- SQLite-consistent backups and explicit board/Durable/worker/budget reconciliation
  checkpoint; independent restore of state, workspace, telemetry and report links.
- Preserve human stop, existing data, spend and receipts through update/rollback.
  Handle schema incompatibility, full disks and failed dependency startup.
- Rewrite installer/configuration/CLI/security/operations/check guides in place;
  remove obsolete live instructions, contradictory Git-helper control row and
  old inventory APIs. Keep generated catalog facts source-owned and historical
  changelog clearly historical. Synchronize UI Guide changes and consumer tests.
- Document pinned source/release versus deployed state, listener/notification
  exposure and independently verified alert delivery.

Acceptance: clean-target restore preserves IDs/ownership/links and does not replay
unknown work or refund budget. Docs links/current command examples pass owning
checks. Catalog reports actual evidence, never fixture-only vendor acceptance.
S15-S20; relevant earlier scenarios retained as regression coverage.

### B12 — Consumer composition and operator rollout handoff

Owner: nixos-host, integration owner. Dependencies: B03, B05-B11; X01/X05/X07.

- Add reviewed pinned Tentaflake/UI/Hostpulse/board composition without resetting
  existing consumer edits or duplicating incompatible Research service protocols.
- Preserve private HTTPS and existing operator access, board state, nonpublic
  listeners and current filesystem; all real identities/secrets stay consumer-side.
- Record measured capacity and selected budgets; no inferred hardware or silent
  installation over an existing host. Existing-host adoption is not the ISO path.
- Evaluate/build exact consumer target with reviewed inputs, then provide the
  human exact target/artifact/effects and rollback/recovery procedure. Do not
  execute activation remotely or through a UI on behalf of the human.

Acceptance: consumer gates/evaluation and authorized system build pass; operator
can inspect complete plan and activate in their terminal. Post-activation metrics,
listeners, containment, private UI and application checks require a separate
record. Without activation, status is prepared, not deployed. S06, S15-S20.

## P2 enterprise and advisory security

### B13 — Scoped MCP connector gateway

Owner: Tentaflake Rust integration package; independent provider adapters.
Dependencies: B04, B06, B10; exact protocol/provider review and X06 for live use.

- Design separate connector identity/credential/egress grants, not a public
  Research extension or arbitrary remote MCP URL. Keep fork five-tool contract.
- Provide configured Drive/Gmail/HubSpot read-only adapters with pinned tool
  schemas, tenant/resource/action ACLs, expiry/revocation, pagination, budgets
  and minimized audit. Prefer explicit reviewed independent packages where viable.
- Write actions are separate capability/approval work; no implicit send/share/CRM
  updates. No actual login, scope widening or credential provisioning by agents.

Acceptance: synthetic provider/MCP servers prove allowed reads and reject wrong
tenant/scope/action/resource, malicious schemas, redirection and revocation;
credentials absent from Nix/agent/UI/logs. Unknown writes never replay. Real
read-only account acceptance is blocked until specifically granted. S04-S06,
S12-S16, S20. Public-web Research remains independently usable.

### B14 — Per-system advisory analyst and optimization proposals

Owner: Tentaflake Durable profile + UI. Dependencies: B06, B09-B10; B13 only for
connector-event review, not permission to access business data.

- One explicitly optional bounded analyst per host consumes redacted evidence.
  Findings link producer/sequence/policy and distinguish observation/inference.
- No container-engine/host-management/credential/raw archive grant. It can propose
  a lifecycle, configuration or code change, never approve/apply it itself.
- Optimization proposal includes baseline evidence, testable prediction,
  acceptance and rollback. No autonomous policy weakening or real benchmark loop.

Acceptance: injected hostile logs obtain no extra capability, no privileged
remediation and no raw-secret access. Model unavailable leaves controls and
operator UI functional. Rule/fixture findings work without paid inference;
classifier quality/cost evaluations are separately authorized, not called proven
injection defense. S01-S08, S10-S16, S19-S20.

### B15 — End-to-end operator acceptance and final evidence

Owner: integration owner/operator. Dependencies: B00-B14 and gates below.

- Synthetic complete path first, then authorized provider/target checks with
  explicit results and unresolved constraints. No hidden mock production fallback.
- Use the acceptance matrix; keep build, activated, runtime and provider evidence
  separate. A report is successful only with expected sources, not a Done label.

Acceptance: all requested matrix scenarios have reproducible passing records,
or the goal stops honestly with named blockers/open items. No untested enterprise
security/24/7/reliability claim.

## Existing verification commands (not run for this plan)

Use current manifests/help before executing; these are inspected entrypoints.
Run in the owning repository's pinned environment through its documented gate.
No arbitrary bare package runtime or independent worker gate duplication.

| Repository | Fast gate | Additional declared relevant gate |
| --- | --- | --- |
| Tentaflake | `just fast` | `just eval`, `just generated-flake`, `just host-api-test`, `just catalog-check`; focused package/VM outputs in `docs/17-builds.md` |
| tentaflake-ui | `project-check fast` | `project-check full`; `bun run test:e2e:demo`, `bun run test:e2e` with the explicitly configured fixture/preview; `bash scripts/check-nix evaluate` |
| team-kanban-board | `project-check fast` or `just check` | `project-check full`; `bun test tests/research.test.ts` in pinned environment for completion/claim regressions |
| Hostpulse | `just fast` | `just ui-build`; separately authorized `nix build --no-write-lock-file .#hostpulse`; system-service/runtime fixture must be added, not invented as existing |
| tentaflake-research | `project-check fast` with documented scratch/TMPDIR setup | `cargo test --locked --offline --test service_rpc`; focused `scripts/check` modes require their documented prepared closures |
| nixos-host | `project-check fast` | `bash scripts/eval-option vps services.tailscale.enable`; affected new options via the same script; authorized consumer build `nix build --no-link .#nixosConfigurations.vps.config.system.build.toplevel` |

Direct Bun/Cargo commands are invoked through the installed toolbox/pinned gate
as required by the workstation; table syntax names the owning check, not a bypass.
Full gates may build outputs: separate their workload and approval from fast
checks. Builds/VMs must be foreground, bounded and sequential; inspect locks and
reuse passing evidence when inputs are unchanged. No `just ci`/ISO build merely
to qualify a document or narrowly changed component.

New scenario tests below must be registered in owning gates. Their identifiers
are proposed acceptance contracts, not executable commands that exist today.

## End-to-end acceptance matrix

| ID | Deterministic scenario | Evidence tier |
| --- | --- | --- |
| A01 | TOML/JSON/form/CLI golden round trip preserves every supported semantic field; removed old runtime source is rejected | Unit/contracts |
| A02 | Prompt creates draft only; changed config/base/pin invalidates approval; approved exact fixture generates, evaluates and builds | Contracts + Nix build |
| A03 | Installer disposable VM reaches private UI with empty stopped inventory; existing-host adoption preserves root/state and needs no destructive installer | Isolated OS/VM |
| A04 | Start admitted declared container; exact UID/mount/resource/egress checks pass; explicit stop prevents restart | Isolated runtime, then target |
| A05 | Authenticated UI creates board-linked research card, one worker claims, progress advances, report with resolvable permitted source provenance is bound before Done | Synthetic whole stack, then authorized live |
| A06 | Crash at every model/Research/worker dispatch boundary yields original result or visible unknown, never duplicate dispatch | Instrumented runtime |
| A07 | Hostpulse CPU/RAM/disk/service/cgroup values match controlled source fixture; stale deadline/disconnect makes UI visibly stale/null; target samples separately confirmed | Collector/HTTP/browser, then target |
| A08 | Network counters reset correctly; policy/tool events display decisions/correlation with explicit gaps and no content/credential canaries | Unit/events/browser |
| A09 | UI/CLI cannot invoke arbitrary commands or access another agent/tenant; all S01-S20 negative cases registered and passing | Contracts + isolated boundary tests |
| A10 | Synthetic Drive/Gmail/HubSpot reads honor grant/resource; writes/revoked account fail; live integration only after scoped consent | Connector contracts, then live account |
| A11 | Advisory analyst cannot mutate policy or lifecycle, even with malicious evidence; unavailable model does not disable safety/management | Synthetic authorization tests |
| A12 | Restore fresh target preserves board/runtime/report IDs and budget/worker receipts without unsafe replay or reset | Isolated restore drill |
| A13 | After human activation, approved private Tailscale route opens UI, A05 succeeds and A07 is observed; no public management listener | Operator/target evidence |

For browser freshness/cancellation/reconnect and natural-language flows, use a
synthetic deterministic model/tool fixture first. Real model calls, cost/quality
benchmarks and enterprise account tests require their own explicit limits and
never run invisibly inside `fast`.

## Implementation qualification and partial handoff

The first run stopped at **20/20 integration cycles**, including failed
verification and fixes. It was not complete or ready for publication,
installation or host activation. The operator then authorized 20 additional
cycles through global cycle 40 and subsequently lifted that cap. Historical
cycle counts remain evidence, not a current stop condition. The overall
B00-B15 and A01-A13/S01-S20 requirements remain unchanged.
The restored route subsequently passed a bounded read-only identity/inventory
check; Research and consumer-host source have not been changed by this
integration owner, and no installation or activation is claimed.

Work was split into independent implementation, read-only review and correction
waves, with one owner running checks after each handoff. Source delivered by
those waves includes the public TOML/compiler cutover, Rust control/connector
boundaries, operator UI surfaces, independent Hostpulse service/projection and
Board acceptance integrity. This does not establish complete prompted
configuration, enterprise MCP deployment, an admitted Pi runner, canonical Board
UI migration, restore or target operation.

### Checks actually exercised

Commands ran through the owning pinned environment using `agent-toolbox nix
develop --no-write-lock-file --command ...`; package builds used the Nix daemon.
These are snapshot-specific results. The first-run results do not qualify later
parallel edits. Continuation cycle 21 established stable diffs for five roots and
repeated the selected core/package matrix; the Board diff changed during that
read-only window and remains held for an independent refreshed review.
There is no current whole-workspace green claim.

| Scope | Command/check | Observed result |
| --- | --- | --- |
| Tentaflake source | `just fast` | First-run pass. Cycle 21 stopped at the unformatted `tests/system-schema.nix`; later fast stages, including current Rust tests, did not run. |
| UI source | `project-check fast` | Passed: Rust transport, Python bridge, frontend tests/types/lint and Nix evaluation. |
| UI artifacts | `project-check full` | Passed: static frontend build, packaged bridge/Rust transport and web package. |
| Board source/artifact | `project-check fast`; `nix build --no-link --no-write-lock-file .#tickets` | Passed, including scope-CAS, immutable-report and delayed-SSE regression cases. |
| Hostpulse source | `just fast` | Passed, including network reset/no-sample and loaded-versus-installed service freshness regressions. |
| Control module/source isolation | `nix build --no-link --no-write-lock-file .#checks.x86_64-linux.control-evaluation`; corresponding `rust-package-sources` check | Passed; not service activation. |
| Real CLI artifact | `nix build --no-link --no-write-lock-file .#tentaflake-cli` | Passed; not a host-system output. |
| Control API/recovery | Explicit `cargo test --locked --offline -p tentaflake-control --test immutable_unix -- --ignored` with immutable fixture, real store CLI, pinned Python and selected UI source | Both tests passed. Real UI adapter and Rust parser handled draft/approval/queued build/lookup. Build executor was synthetic; no host build or activation. See [control prerequisites](20-system-control.md#cli-and-verification). |
| Shared compiler corpus | `nix build --no-link --no-write-lock-file .#checks.x86_64-linux.system-schema` | Cycle 21 passed the current Nix shared-fixture corpus and host projection; the historical eager `extraVolumes` lookup failure was not reproduced. This is not a current Rust-corpus result. |
| Control package | `nix build --no-link --no-write-lock-file .#tentaflake-control` | Cycle 21 reproduced `private filesystem operation refused` in the Nix-build fixtures: four unit tests passed and eleven failed. No production filesystem check was relaxed. |
| Module/adapter gate | `just eval` | First-run failure referenced the untracked isolation helper. The current helper is index-visible, but this gate has not yet been repeated for the continuation. |
| Actual installer generation | `just generated-flake` | Cycle 21 reproduced the positive fixture's balanced/strict private-management and SSH-policy assertion failures. Refusal/preservation cases ran first; they are not a passing happy path. |
| Hostpulse artifact | `nix build --no-link --no-write-lock-file .#hostpulse` | Cycle 21 passed the actual current independent package with the activity/resource inputs present. It does not qualify a running system service, distinct-UID projection, browser or 24/7 target. |
| Browser | `bun run test:e2e` | Blocked before UI assertions: Chromium/ALSA library loading required unavailable `GLIBC_2.43`. Opt-in live specs were not enabled. The temporary loopback static server exited and its port was checked free. |
| Security evidence | Offline `agent-security-scan privacy . --tracked` and `deps .` | Not an all-clear. Privacy had synthetic/existing pattern matches and source-inventory omissions; dependency coverage was incomplete without an explicitly provisioned OSV runtime database. No online refresh/account operation was performed. |

The historical matrix above did not include a local VM run. The later synthetic
no-NIC workload/projection VM pass is recorded in O02-O03; it is not UI HTTP,
browser or target acceptance. No real provider/model/enterprise-account workload,
VPS collector acceptance, installer disk operation, production service change,
commit, push or activation is claimed by these results. A passing package or
synthetic fixture is not a 24/7 target or prompt-injection-detection guarantee.

### Prioritized resumption backlog

1. **R00 / X02: establish a coherent reviewed source snapshot.** Coordinate the
   parallel Pi/connector/activity/isolation work and lockfile changes first.
   Preserve unrelated edits; review missing files before including them in Nix.
   Re-run gates for that exact integrated state, not a mixture of old results.
2. **R01 / B01-B03: close compiler and installer failures.** Make unknown-field
   rejection precede descriptor access; retain the negative corpus. Correct the
   actual generated-host happy path and explicit denied-management cases while
   retaining balanced isolation, private transport and SSH authorization.
3. **R02 / B04-B05/B08: close package failures.** Diagnose protected fixture
   ancestry/ownership in the Nix build environment without bypassing production
   checks. Integrate the reviewed Hostpulse activity inputs and repeat package
   builds and distinct-UID service/projection acceptance.
4. **R03 / B04-B05/B09: qualify the browser and complete configuration UX.**
   Use a mutually compatible pinned browser/library environment, not a disabled
   sandbox or arbitrary host browser. Then run the actual draft/diff/approval/
   build/freshness flows. Natural-language draft assistance and enabled system
   categories still require integrated source and tests; a JSON editor or
   disabled-only placeholder is not their completion.
5. **R04 / B06-B07/B10/B13-B14: integrate runtime authority.** Review and qualify
   the parallel real-SDK work, no-replay ledger, exact Research protocol and
   mediation. Finish Board-authority convergence, activity provenance, MCP/
   credential deployment and advisory-only analyst wiring. Reuse qualified
   existing work; do not silently replace it with another model loop.
6. **R05 / B11-B12: restore and target preparation.** Complete preservation-
   checked migration/backup/fresh-target restore, extend the private bounded
   target inventory into consumer/state/resource review and prepare the actual
   consumer configuration and reviewed build output.
7. **R06 / B15/O01-O09: operator acceptance.** Resolve Board redistribution
   licensing, approved Tailscale access, provider/account/workload budgets and
   human activation. Then perform A13. None of these outcomes is presumed.

The operator lifted the earlier cycle cap; current source ownership and exact
inputs still require coordinated review. Historical accounting remains intact,
and no open requirement is removed. The additional work allowance does not
authorize real provider/model/account workloads, publication, production changes
or agent-driven activation; those retain their separate authorization boundaries.
