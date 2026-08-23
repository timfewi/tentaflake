# Secure installed-host quickstart

Installed hosts default to the fail-closed `balanced` profile. This guide
verifies that baseline. An agent remains offline unless its exact container
name has an explicit broker policy. The optional broker path grants narrow
model/fetch APIs, never general internet access.

## 1. Install the external management policy

Replace the placeholder identity in `docs/tailscale-policy.example.json`,
validate it in the Tailscale admin console, and install it before relying on
Tailscale as the only management path. The policy must restrict both network
grants and Tailscale SSH to the operator and `tag:agent-host`.

The default tailnet allow-all policy is not acceptable. Do not enable Funnel
or publish an agent API with Serve. Protect the identity provider with
hardware-backed MFA; plan Tailnet Lock with at least two signing nodes.

## 2. Declare a stopped capsule

Copy the current example:

```bash
sudo cp \
  /etc/nixos/my-agents.nix.example \
  /etc/nixos/my-agents.nix
```

The example contains Hermes and ZeroClaw capsules with
`autoStart = false` and a matching disposable worker for each. Remove both a
runtime and its worker entry when you do not need it. Do not add `envFile`,
`agenixFile`, `hostPort`, `servePort`, host networking, or literal secret
values under balanced; evaluation rejects them.

## 3. Evaluate and build without activation

From `/etc/nixos`, resolve the installed hostname and build its toplevel:

```bash
hostname
nix build \
  .#nixosConfigurations.<hostname>.config.\
system.build.toplevel \
  --no-link
```

Review any assertion and the source diff. A successful build proves the
candidate closure, not the live host.

## 4. Activate only in an approved window

Activation is an explicit host mutation. Once the exact host and build result
are reviewed, the operator may run:

```bash
sudo nixos-rebuild switch \
  --flake /etc/nixos#<hostname>
```

Tentaflake never performs this step merely because evaluation succeeded.

## 5. Verify the activated posture

```text
tentaflake status
tentaflake doctor --security
tentaflake doctor --security --json
sudo systemctl cat \
  docker-hermes-coding.service
```

For Podman, the unit prefix is `podman-`. A balanced capsule should show
`runsc`, non-root user, cap-drop-all, read-only root, no-new-privileges,
hardened tmpfs, and resource flags. Without a broker it also shows
`network=none`, and the security doctor reports `TFSEC-013`. With a broker it
shows one internal network, runtime virtual credentials, and no direct route.
It should not report `TFSEC-020`; that finding means the corresponding
disposable worker boundary is absent. `TFSEC-021` means its persistent
workspace has no managed hard size ceiling.

`autoStart = true` is intentionally rejected under `balanced` until the same
container key has an enabled broker, worker, and workspace quota. Keep the
controller stopped while those policies are incomplete or while migrating an
existing workspace.

## 6. Keep the agent stopped

Do not place real OpenAI, Anthropic, OpenRouter, GitHub, Firecrawl, or
infrastructure credentials in the capsule. Without a scoped broker
declaration, a balanced agent cannot perform external model or web calls and
should remain stopped. To grant narrow access, follow
[brokered egress](12-brokered-egress.md), build, review the generated units
and firewall rules, then approve activation separately.

For a trusted development-only compatibility host, the intentional escape is:

```nix
tentaflake.security.profile = "dev";
```

That permits legacy direct credentials and networking but is explicitly not a
security boundary for untrusted 24/7 agents. There is no silent fallback from
balanced or strict.

## Next reading

- [Security profiles and migration](10-security-profiles.md)
- [Tailscale management policy](11-tailscale-management.md)
- [Operations and recovery](07-operations.md)
- [Agenix boundaries](04-agenix-secrets.md)
- [Brokered egress](12-brokered-egress.md)
- [Disposable worker and approval](13-disposable-worker.md)
- [Persistent workspace quota](14-workspace-quota.md)
- [Threat model](15-threat-model.md)
