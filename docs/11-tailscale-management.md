# Tailscale management-plane policy

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
evaluation. The security doctor checks local `tailscale serve status --json`
for active Serve/Funnel state and reports a blocked local API as unknown, but
the operator must still inspect and validate the remotely active policy;
neither restrictive grants nor SSH rules are inferred from
`tentaflake.tailscale.enable = true`.


## Enrollment and local preferences

The template applies `--ssh`, the host name and `tag:agent-host` through
`services.tailscale.extraSetFlags`. NixOS creates `tailscaled-set` even without
`authKeyFile`; `extraUpFlags` only runs during automatic key enrollment and must
not be used for these persistent preferences. This does not enroll a node or
validate the remote policy. An operator must still complete enrollment and
verify the live preferences and access from a separate authorized session.

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
| [P1: Generic private management contract](https://github.com/timfewi/tentaflake/issues/106) | `modules/security.nix` requires `tentaflake.tailscale.enable`; the security manifest and CLI also carry a Tailscale-specific boolean. A different private transport cannot satisfy balanced policy. | Define explicit management transport and private SSH policy; keep current defaults compatible, reject public access, version the manifest and make diagnostics transport-aware. Disabling Tailscale must not silently disable the management requirement. |
| [P1: Optional Headscale control-server configuration](https://github.com/timfewi/tentaflake/issues/107) | `modules/tailscale.nix` has no first-class login server, enrollment credential, tag or SSH-mode options. Consumers can override upstream NixOS options, but enrollment is not a template-tested workflow. | Select a control server explicitly while retaining the Tailscale client. Keep runtime credentials out of the store; test enrollment, logout, restart and unreachable server. Tags must be configurable and constrained by remote policy. No automatic control-plane hosting. |
| [P1: SSH policy compatible with the chosen control plane](https://github.com/timfewi/tentaflake/issues/108) | Balanced forbids every OpenSSH service and assumes Tailscale SSH. The example's `check` action and Tailnet Lock guidance target the hosted Tailscale policy. | Verify the chosen Headscale version's SSH capabilities, or support key-only OpenSSH restricted to the private interface. Test public denial and operator access; do not assume hosted policy features work on another control plane. |
| [P1: Management readiness and recovery evidence](https://github.com/timfewi/tentaflake/issues/109) | The doctor checks the declared enable flag and Serve/Funnel state; it does not verify enrollment, a usable operator path or the remote grants. | Report declared, enrolled, reachable and remotely unverified states separately with bounded probes. Test offline control plane, reboot and credential expiry; retain console/recovery access. Never mark remote authorization verified from a local flag. |
| [P1: Provider-independent Research VPN adapter acceptance](https://github.com/timfewi/tentaflake-research/issues/4) | Research already accepts an external observation adapter. Its optional reference observer checks interface-up, an IPv4 route across routing tables and a root-owned firewall marker; the marker is not verification of live rules or tunnel identity. | Document a reusable adapter contract and test selected tunnel/exit-node configurations for IPv4, IPv6, DNS, policy routing, tunnel loss and stale observations. Keep the independent UID firewall fail-closed. The observer's `region` is an operator assertion. |

The manual-enrollment preference bug above is addressed in
[PR #105](https://github.com/timfewi/tentaflake/pull/105), with a regression assertion
in `tests/module-eval.nix`. The adapter items remain follow-up work.
No enrollment, ACL publication, VPN switching or host activation is performed
by this review.


Verification checkpoint: the new manual-enrollment assertion failed before the
fix because `extraSetFlags` was empty. After the change, `just fmt` and `just fast`
passed, including Nix lint/formatting, Rust checks, module assertions, policy and
adapter checks, read-only flake evaluation and generated-installer-flake checks.
The regression verifies the declared `tailscaled-set` wiring; live enrollment,
SSH access and unit behavior on an installed host were not exercised. No VM suite
or host activation was run. Both Research input and lockfiles remain unchanged.

The reviewed Research pin update is tracked separately in
[issue #110](https://github.com/timfewi/tentaflake/issues/110); it requires upstream review and
affected integration acceptance before deployment.
