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
