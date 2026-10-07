# Private management contract and Tailscale policy

## Host contract and migration

Configure private operator connectivity and SSH authorization separately from
agent capabilities and Research Internet egress:

```nix
tentaflake.management = {
  enable = true;
  transport = "tailscale";
  ssh.policy = "tailnet-policy";
};
```

These are the existing defaults. `tailscale` is the only currently reviewed
transport, using the Tailscale client; the contract does not require a hosted
control-plane account or automatically configure Headscale. Other transport
names fail validation until their implementation and acceptance are added.
`tailnet-policy` selects Tailscale SSH and requires the operator to maintain
restrictive remote grants and SSH rules. `ssh.policy = "disabled"` is available
only for development configurations; it cannot satisfy `balanced`.

The existing `tentaflake.tailscale.enable` option is an alias for
`tentaflake.management.enable`. Existing consumers retain their behavior; migrate
declarations to the new name when convenient. The two names are the same option,
not independent switches, so do not give them conflicting values. Disabling
management through either name still fails `balanced`; it does not waive the
private management requirement. The public OpenSSH module remains forbidden.

The internal, read-only `management.capabilities.privateConnectivity` is derived
from the enabled contract and actual configured Tailscale service.
`management.capabilities.sshAuthorization` is derived from that connectivity,
the selected policy and the final SSH flags after overrides. It becomes
`disabled` if SSH is not enabled in `extraSetFlags`, if a disabling SSH argument
appears in the set/up flags, or if an argument separator prevents reliable
interpretation. Balanced requires both configured capabilities. They cannot be
set by an agent or replaced with operator-written readiness booleans.

These capabilities describe configuration. They do not establish enrollment,
live connectivity, operator-tested access, remotely effective grants or a usable
recovery path. Host configuration remains operator-owned; importing agent data
does not choose transports, control servers, firewall policy or credentials.
Keep console access and a separately tested operator session before any
explicitly authorized management change.

The generated `/etc/tentaflake/security.tsv` now starts with `manifest` and
version `2`, separated by a tab. Its `management` record carries the transport,
enabled state, configured private connectivity and derived SSH authorization.
The host record no longer carries the Tailscale-specific enable boolean. The CLI
reads legacy unversioned manifests but treats their management contract as
unknown; a missing version-2 management record is also unknown. Under balanced,
`TFSEC-038` reports that gap, `TFSEC-019` reports disabled/missing private
connectivity, and `TFSEC-039` reports disabled configured SSH authorization.
Unsupported versions/transports/policies and duplicate records fail parsing.
Regenerate the manifest through an authorized host update; never hand-edit it
to manufacture evidence.

## Remote authorization

Tailscale connectivity is private transport, not authorization by itself. A
new tailnet's permissive default policy must be replaced before treating it as
the only management path.

`tailscale-policy.example.json` is syntactically valid JSON and separates:

- a network grant from one operator group to TCP 22 on `tag:agent-host`; and
- a Tailscale SSH rule with `action: check`, including privileged root access.

Replace `operator@example.invalid` with one controlled identity and validate
the policy in the Tailscale admin console before saving it. Add other ports
only for an explicitly authenticated operator service. Do not use Funnel for
a secure host, and do not automatically publish agent APIs with Tailscale
Serve. Tailnet-visible is still reachable by every identity the policy allows.

Protect the identity provider with phishing-resistant hardware-backed MFA.
Enable Tailnet Lock only after provisioning at least two independent signing
nodes and testing the recovery procedure. Keep auth keys and lock signing keys
out of Nix expressions and the Nix store.

The template cannot confirm the remote admin-console policy from local Nix
evaluation. For an enabled Tailscale declaration or an unknown legacy/incomplete
management contract, the security doctor checks local `tailscale serve status --json`
for active Serve/Funnel state and reports a blocked local API as unknown, but
the operator must still inspect and validate the remotely active policy;
neither restrictive grants nor SSH rules are inferred from an enabled management
contract or its configured capabilities.


## Enrollment and local preferences

The template applies `--ssh` and the host name through
`services.tailscale.extraSetFlags`. NixOS creates `tailscaled-set` even without
`authKeyFile`. `--advertise-tags` is accepted only by `tailscale up`, so
`extraUpFlags` retains the tag and these preferences for automatic auth-key
enrollment. NixOS does not apply `extraUpFlags` without `authKeyFile`.

For a new manually enrolled node, supply the tag explicitly using the configured
host name:

```sh
sudo tailscale up --advertise-tags=tag:agent-host --ssh --hostname='<host-name>'
```

On an already configured node, preserve all existing non-default preferences;
follow the CLI's required flag list rather than resetting them. Local settings
do not enroll a node or validate the remote policy. Verify the assigned tag,
live preferences and access from a separate authorized session.

## Modular management review (2026-10-03)

Requirement: keep Tentaflake a provider-independent host for agents running
continuously. These are source-review findings and issue candidates, not
implemented adapters or verified production networking.

Management connectivity and Research Internet egress are separate capabilities.
Tailscale/Headscale controls node membership and operator reachability. Research
uses the explicitly configured `services.secureResearch.vpnInterface`, resolver
policy, UID firewall and root-owned readiness leases; it neither creates the VPN
nor chooses the control server. A management tailnet does not automatically
provide an Internet exit node.

| Priority / proposed issue | Current evidence | Acceptance criteria |
| --- | --- | --- |
| [P1: Generic private management contract](https://github.com/timfewi/tentaflake/issues/106) | The typed management/SSH contract, legacy alias and version-2 manifest describe configured capabilities. Tailscale is the only reviewed transport; enrollment and remote policy remain unverified. | Preserve current consumers and public-access refusal. Authorized runtime fixtures still need to prove public-access denial and operator access before acceptance; source evaluation alone does not close this issue. |
| [P1: Optional Headscale control-server configuration](https://github.com/timfewi/tentaflake/issues/107) | The management contract selects Tailscale SSH, but login-server, enrollment-credential and configurable-tag support remain separate. Consumers can override upstream NixOS options; Headscale enrollment is not a template-tested workflow. | Select a control server explicitly while retaining the Tailscale client. Keep runtime credentials out of the store; test enrollment, logout, restart and unreachable server. Tags must be configurable and constrained by remote policy. No automatic control-plane hosting. |
| [P1: SSH policy compatible with the chosen control plane](https://github.com/timfewi/tentaflake/issues/108) | Balanced forbids every OpenSSH service and assumes Tailscale SSH. The example's `check` action and Tailnet Lock guidance target the hosted Tailscale policy. | Verify the chosen Headscale version's SSH capabilities, or support key-only OpenSSH restricted to the private interface. Test public denial and operator access; do not assume hosted policy features work on another control plane. |
| [P1: Management readiness and recovery evidence](https://github.com/timfewi/tentaflake/issues/109) | The doctor checks the typed configured contract and Serve/Funnel state; it does not verify enrollment, a usable operator path or the remote grants. | Report declared, enrolled, reachable and remotely unverified states separately with bounded probes. Test offline control plane, reboot and credential expiry; retain console/recovery access. Never mark remote authorization verified from a local flag. |
| [P1: Provider-independent Research VPN adapter acceptance](https://github.com/timfewi/tentaflake-research/issues/4) | Research already accepts an external observation adapter. Its optional reference observer checks interface-up, an IPv4 route across routing tables and a root-owned firewall marker; the marker is not verification of live rules or tunnel identity. | Document a reusable adapter contract and test selected tunnel/exit-node configurations for IPv4, IPv6, DNS, policy routing, tunnel loss and stale observations. Keep the independent UID firewall fail-closed. The observer's `region` is an operator assertion. |

The manual-enrollment preference bug above is addressed in
[PR #105](https://github.com/timfewi/tentaflake/pull/105). Its regression covers
manual and auth-key configurations and parses the actual flags with the pinned
Tailscale CLI. The adapter items remain follow-up work.
No enrollment, ACL publication, VPN switching or host activation is performed
by this review.


Verification checkpoint (2026-10-04): the pinned Tailscale 1.98.8 parser rejects
the former `set --advertise-tags` invocation before contacting a daemon. The
module-evaluation regression reproduces that failure with the generated flags;
`--help` validates parsing without enrollment. All fast-gate checks pass with
the corrected flags. Live enrollment, SSH access and
installed unit behavior require separate acceptance. No VM suite or host
activation was run; Research and lockfile pins remain unchanged.

The reviewed Research pin update is tracked separately in
[issue #110](https://github.com/timfewi/tentaflake/issues/110); it requires upstream review and
affected integration acceptance before deployment.

## Contract acceptance limits

Focused option and CLI regressions cover defaults, the old alias, service/SSH
overrides, unsupported selections, public OpenSSH refusal, versioned parsing and
legacy/missing management evidence. These are source-level checks. Issue
[#106](https://github.com/timfewi/tentaflake/issues/106) remains open for authorized
runtime fixtures proving public-access denial and existing-consumer operator
access. VM/runtime workloads were excluded from this lightweight-only run;
no enrollment, remote policy change, host activation or runtime acceptance is
claimed. Headscale enrollment and alternative private SSH remain separate
[#107](https://github.com/timfewi/tentaflake/issues/107) and
[#108](https://github.com/timfewi/tentaflake/issues/108) work.
