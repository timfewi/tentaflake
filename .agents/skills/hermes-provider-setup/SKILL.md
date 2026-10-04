---
name: hermes-provider-setup
description: Configure or diagnose Hermes model access through Tentaflake's host-held LLM broker credentials and exact model policy.
version: 1.1.0
platforms: [linux]
metadata:
  hermes:
    tags: [provider, model, broker]
    category: devops
    requires_toolsets: [terminal]
---

# Hermes model access

1. Resolve the container, pinned image and security profile. Read
   [brokered egress](../../../docs/12-brokered-egress.md) and
   [configuration](../hermes-config-manager/SKILL.md).
2. Under balanced, configure its exact `tentaflake.broker.agents` entry:
   HTTPS `llm.upstreamBaseUrl`, runtime `providerCredentialFile`, exact
   `allowedModels` and reviewed prices/budgets. systemd loads real keys only
   into the host broker; the capsule receives a virtual key and broker endpoint.
3. Select an allowed model through declarative Hermes `settings`. Verify
   provider/base-URL behavior against the pinned runtime so requests reach the
   broker. Do not run in-container OAuth/key setup, direct fallbacks or
   auxiliary model calls to another endpoint.
4. If the verified client needs SSE, explicitly enable that broker's
   `llm.streaming.enable` (default false). Retain byte/event limits, deadlines,
   no replay and conservative reservation. Client timeouts cannot relax policy.
5. Evaluate, review and build. Activate within existing operator authorization;
   verify broker readiness and a minimal model exchange when provider usage is
   authorized.

## Diagnose and rotate

Check the exact broker unit, bounded logs, `/healthz`, model spelling, endpoint,
budgets and streaming policy. `/healthz` checks local credential/policy/audit
readiness, not upstream TLS, model access or billing.

Rotate values through the host secret mechanism and restart only the exact LLM
broker when authorized. Virtual keys survive broker restarts. Never put keys
in command arguments, settings, JSON, logs or Git. See
[Agenix](../../../docs/04-agenix-secrets.md).

Direct agent credentials require explicit dev and a reviewed runtime channel.
Provider catalogs, context limits and OAuth behavior vary by image; inspect
matching help instead of assuming universal support. Provider-hosted web tools
remain forbidden on the secure path; web uses
[Research](../../../docs/16-research.md).
