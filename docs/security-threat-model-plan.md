# Security threat model for the proposed integrated system

Status: design requirements, not security acceptance. Read with the
[architecture](system-architecture-plan.md), [backlog](implementation-backlog.md)
and existing [host threat model](15-threat-model.md). This plan extends that
model to configuration generation, UI, monitoring, enterprise connectors and
Pi Durable. It does not imply these features exist.

## Security objective, adversary and trusted base

Assume an agent, model output, retrieved website, email, CRM entry, repository,
MCP response or dependency can be hostile. Compromise of one agent must not grant
host administration, another agent's state, provider/connector credentials,
unrestricted egress or authority to change the desired system. Host and account
owners retain access, revocation and recovery. No component may obstruct an
explicit stop or removal.

Assets: approved configuration and build graph; host kernel/boot/container engine;
operator sessions and recovery; enterprise tenant/account data and credentials;
per-agent workspace/conversations; board tasks and results; budget/worker/build
receipts; private telemetry; backup integrity and service availability.

Trusted base: operator, activated NixOS policy, reviewed Rust services and their
protocols/dependencies, Nix daemon and sandbox, kernel/systemd/filesystem/gVisor,
authorized Tailnet policy, credential provisioning, provider identity and backup
operators. Rust reduces some memory-safety risks; it does not eliminate logic,
authorization, unsafe dependency, kernel or supply-chain failures.

The existing balanced capsule shares the host kernel. Docker/Podman management
and root remain powerful; same-UID access is not isolation. `strict` is currently
unimplemented/fail-closed; `dev` is not a fallback. This is not yet a proven
hostile-multitenant enterprise platform. Multi-user identity, role/resource grants
and tenant isolation require separate acceptance before claiming that scope.

## Principals and maximum intended grants

| Principal | Permitted scope | Must not receive |
| --- | --- | --- |
| Viewer | Authorized inventory, minimized metrics and task/result views | Configuration writes, credentials, raw archives, lifecycle/build control |
| Configuration editor / prompt helper | Versioned draft proposals | Approval, arbitrary Nix/shell, activation, secret resolution |
| Build approver / coordinator | Exact reviewed config/input/target hash, bounded build receipt | Arbitrary paths/flags, unrestricted commands, agent-selected targets or activation |
| Lifecycle operator | Fixed actions on exact admitted declared instances | Container-engine socket, arbitrary systemd units or exec |
| Agent / Durable runner | Own state, exact model/Research/connector grants and worker interface | Host/UI management sockets, peer state, real upstream credentials |
| Offline worker | Approved bounded local execution with workspace snapshot | Network, host secrets, devices, privileged runtime control |
| Research relay/service | Assigned caller and public-web job grants | Enterprise accounts, policy changes, replay of dispatched calls |
| Enterprise connector | Exact tenant/account/resource/action grant | Arbitrary MCP origin, scope widening, other tenant data |
| Hostpulse + projection | Consented local observations and minimized read-only export | Host control, unrestricted cross-UID archives, packet/content capture by default |
| Security analyst | Redacted evidence summaries and recommendations | Remediation/activation, policy edits, enterprise secrets, raw global logs |
| Human host operator | Reviewed installation, activation, credential and incident actions | Authority is never delegated implicitly by a prompt or model decision |

These roles are proposed, not present enterprise RBAC. The present UI's shared
operator token cannot substantiate distinct identities or tenant grants.

## Trust boundaries and enforcement

1. **Browser -> application:** authenticate identity, validate origin/CSRF and
   schema/limits. Enforce resource authorization server-side on every call.
   Private Tailscale HTTPS is transport, not application authorization.
2. **Natural language -> desired configuration:** model output is a draft only.
   Typed Rust validation, category allowlists, semantic diff and human approval
   are outside model authority. No arbitrary root/Nix instructions.
3. **Application -> build/host management:** different narrow sockets/capabilities;
   exact peer identity, canonical instance/target, durable intent and stale-base
   rejection. Do not mount host-control sockets in agent containers.
4. **Host -> capsule/worker:** shared containment, mounts, identities, resource
   bounds, exact broker/relay sockets and no direct egress. Verify actual native
   tools use the boundary; a configuration declaration is insufficient.
5. **Agent -> external provider/connector:** host-held credentials, exact grants,
   bounded request and output, public/private destination policy, revocation and
   no automatic replay of uncertain sends/writes.
6. **Workload -> monitoring -> UI/analyst:** export minimized typed metadata with
   freshness and provenance. Treat messages/labels as data, never executable UI,
   trusted instructions, approval or causal proof.
7. **Board -> runtime -> artifacts:** revision-bound claim, stable correlation,
   no second board writer, report hash stored before completion. A heartbeat or
   status is a report, not independent proof that work succeeded.
8. **State -> backup/restore:** application-consistent snapshots and explicit
   reconciliation. Missing receipts or rollback cannot authorize redispatch or
   budget reset. Preserve human recovery copies and stop all relevant writers.

## Abuse cases and deterministic security tests

The identifiers below are required test scenarios, not already existing tests.
Use synthetic identities, clocks, endpoints and credentials. Routine tests must
not read real mail, telemetry archives, tokens or production agent transcripts.

| ID | Abuse/failure | Required enforcing control and observable test |
| --- | --- | --- |
| S01 | Prompt asks to disable sandbox, add arbitrary Nix or expose a secret | Parser rejects unsupported/secret-bearing fields; proposal cannot invoke a build before exact human approval. Prompt-helper tests observe zero privileged dispatches. |
| S02 | Approved draft changes or policy/input graph changes before build | Bind canonical config, base, compiler, policy and pins to approval. Mutate each independently; build request returns a conflict with zero new builds. |
| S03 | Browser sends arbitrary command, unit, path, flake or Nix flag | Typed coordinator/lifecycle APIs reject each extra/invalid field and canonical-ID mismatch. No generic exec endpoint or client-selected socket. |
| S04 | CSRF, stolen lower-role token, cross-tenant request or expired session | Same-origin/CSRF, session expiry and resource-scoped auth tests reject requests; logout invalidates session and privileged UI state. Tailnet membership alone grants nothing. |
| S05 | Hostile MCP/email/site instructs agent or analyst to widen grants | Tool/connector authorization uses trusted policy, never content. Fixtures can produce misleading text but obtain zero unauthorized operations or configuration approvals. This tests enforcement, not universal injection detection. |
| S06 | Agent tries LAN/metadata/direct provider/other-agent/socket access | Isolated runtime tests deny each route and mount. Exact allowed broker/relay still works. No fallback to direct network, remote MCP or dev profile. |
| S07 | Model/Research connection breaks after dispatch | Keep partial/unknown outcome and held accounting; restart causes zero repeated sends. Reconnect performs fresh protocol/UID checks for future calls only. |
| S08 | Worker dies after durable claim or approval response is lost | Exact ID/digest retains existing receipt; recovery never redispatches a running claim. A recorded approval cannot be rendered as completed execution. |
| S09 | Board claims race or forged completion/status update | One winner for a revision; stale/cross-owner writes fail. Done requires the intended report/source hash policy; generic PATCH cannot bypass it. |
| S10 | Same-user projection exposes Hostpulse admin/content operations | UI peer can read only approved metadata; ForgetSession/import/clock acknowledgement and raw journal/native content are denied. Wrong UID fails even when the socket path is known. |
| S11 | Telemetry is absent, reset, forged, delayed or dropped | Inject clock/counter resets, disconnect and sequence gaps. UI renders null/no-sample/stale/partial, not healthy zero; unproven workload mapping remains unknown. |
| S12 | Tool/log content injects HTML, secrets or unbounded cardinality | Encode output, apply CSP and size/cardinality caps, exclude bodies by default. Credential canaries never appear in public events, browser errors, Nix outputs or routine logs. Redaction is not a guarantee for opted-in raw capture. |
| S13 | Wrong tenant/resource, revoked OAuth grant, malicious MCP server or redirect | Gateway rejects wrong audience/account/action/resource and unreviewed origins; token expiry/revocation closes access. Provider adapters cannot redirect credentials to another origin. |
| S14 | Send/share/CRM mutation is retried after uncertain provider result | Intent digest, account/resource/action and human approval are stable. Restart reports unknown rather than resending; conflicting retry is rejected. Read grants cannot authorize writes. |
| S15 | Approved runtime image or dependency is replaced | Build/activation preflight binds actual pinned artifacts and reviewed signer policy where enabled. Changed digest/pin/signature blocks admission. Pinning alone is not a benign-code claim. |
| S16 | Resource, task, connection, event or build queue exhaustion | Existing limits plus admission caps fail boundedly; operator control and reserve disk/memory remain available in isolated tests. Unknown workload cannot consume an unbounded per-label series. |
| S17 | Unsafe symlink, hidden mount data, corrupt DB or concurrent writer | Descriptor/path/ownership guards and exclusive state writer reject startup/mutation; files outside target remain unchanged. Failed migration retains original state and recovery path. |
| S18 | Old backup/code rollback loses spend, approval or task receipts | Restore drill preserves IDs or explicitly pauses/reconciles uncertainty. No automatic replay, new credentials, budget initialization or destructive schema downgrade. |
| S19 | Operator explicitly stops/removes an agent while timers are active | Service/analyst/Durable timers cannot restart it. Existing workers retain truthful interrupted outcomes; stop does not grant approval or erase evidence. |
| S20 | Build succeeds but collector/provider/app is unhealthy | UI separates built, activated, process-running, dependency-ready and application-tested. No green production state from fixtures, provider listing or a model's assertion. |

## Network/tool visibility and privacy

Hostpulse currently has no network accounting or container discovery. Planned
interface counters measure volume, not destination/permission. Policy events
must originate from brokers, Research, connectors and worker/runtime boundaries;
include producer identity, schema/policy revision, operation/correlation ID,
time and decision/outcome. Mark missing coverage explicitly. Do not promise
complete connection tracing, decrypted payload capture or detection of every
attempt from these surfaces.

Prompts, tool arguments/results, email/Drive bodies, CRM records, URLs and even
labels may contain secrets or personal data. Default integration exports counts,
categorical outcomes and opaque references, not bodies or arbitrary query text.
Separate log-detail grants, event retention, export and erasure procedures. Native
full capture and journal body collection require explicit consent and are not
silently enabled for the security model. Hostpulse's archive is plaintext;
owner-only permissions do not protect it from root or same-UID compromise.

Use one optional advisory analyst per host, with distinct state/identity and
bounded request/token/time budgets. It can misclassify malicious content and be
prompt-injected itself. Model findings remain untrusted evidence-linked proposals.
Policy and operator review enforce actions regardless of classifier output;
missed detections cannot authorize anything. Runtime/classifier quality needs a
separately approved evaluation corpus and model-run budget, not a routine gate.

## Enterprise connector requirements

Drive, Gmail and HubSpot need their own reviewed protocol/server/licensing and
provider contracts. Public Research is not a generic authenticated HTTP proxy.
Specify tenant/account, OAuth audience and exact scopes, resources and methods,
credential delivery, refresh/revocation, egress destinations, data retention,
maximum page/body size, concurrency, deadlines, costs and audit before admission.
Server updates and tool schema changes are reviewed capability changes.

Start synthetic, then read-only owner-consented accounts. Scope expansion,
login, real credential provisioning, mail sending, external sharing and CRM
writes are separate grants. Human approval binds the exact action intent; a
retrieved document, board card or model-generated tool call cannot supply it.
Revocation must be enforced by the gateway, not just hidden from the UI.

## Existing evidence gaps and documentation corrections

- VPS inventory could not connect; no live kernel, metrics, containment, Tailnet,
  Research egress, credential or restore evidence exists from this run.
- Pi is not runnable. Its pinned retry/recovery policy must support Tentaflake's
  no-replay contract before admission; default upstream durability is insufficient.
- Hostpulse is unqualified on the target; private same-UID socket and session-only
  service cannot be treated as a remote 24/7 monitoring interface.
- Research fork v1/five tools and upstream v2/seven tools must not be mixed;
  Tailscale management is not accepted Research Internet-exit evidence.
- Board completion has a generic-PATCH bypass of report binding; issue changes
  lack the required CAS semantics. Board licensing also needs clarification.
- UI HQ task authority and external board authority require an explicit cutover,
  not two writable copies. Current shared-token auth is not enterprise tenancy.
- Existing `15-threat-model.md` has a control-table row referring to a host Git
  helper while operations prohibit host auto-push. Implementation docs cleanup
  must resolve that contradiction in place; do not recreate a credentialed helper.

No vulnerability exploitation, static scan, paid model test, installation or
runtime acceptance was performed for this plan. Source evidence supports design
constraints only. An unavailable check remains unknown, never passed.

## Residual risks and response

Accepted design limits include shared-kernel/gVisor risk, compromised trusted host
services, data destruction within an agent's own writable state, disclosure to an
allowed external provider, sampling/telemetry gaps, classifier failures, provider
billing uncertainty, malicious supply chain, operator error and host-wide outages.
These need explicit deployment acceptance; a dashboard or security model cannot
make them disappear.

Incident response preserves bounded evidence and human recovery. Deterministic
controls deny ungranted actions; an advisory alert requests operator review.
Stopping another process, isolating a network, rotating credentials, changing
access policy or contacting third parties requires separate scoped authorization.
Do not erase evidence or perform speculative autonomous remediation. Keep an
independent operator access path, known-good code generation and verified backup;
activation/recovery stays in the human's terminal.
