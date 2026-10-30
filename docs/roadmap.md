# Product direction

Tentaflake is a declarative, runtime-agnostic host: one `agents.json` source,
one implementation per adapter, shared containment and explicit operator
activation. It requires neither a shared agent runtime nor a mandatory web UI.
This checkout is not a new published release; tags define released behavior.

## Priorities

1. Keep JSON onboarding and installation simple without hiding Nix host policy,
   changing defaults or silently relocating state/accounting.
2. Complete actual vendor startup, model transport, Research discovery,
   execution mediation, lifecycle and restore acceptance for each runtime.
   Source fixtures alone do not establish support. OpenClaw/Pi remain refusal
   scaffolds; generic OCI remains offline without accepted model/Research hooks.
3. Improve bounded application readiness and diagnostics without replaying work
   or overriding explicit operator stops.
4. Add adapters through their own reviewed image, native hooks and evidence,
   not new shared security implementations or runtime-prefixed host services.

Current support is in [the generated catalog](agent-adapters.md#support-and-upstream-evidence).
The [host API](19-web-ui.md) is an optional interface to declared instances;
native chat and external task ownership are separate integrations.

## Inspiration and evidence limits

The common design separates agent commands from trusted policy/credentials.
OpenShell is not a required backend: its NixOS/gVisor/broker/Research compatibility
has not been accepted. A future backend requires its own isolation evidence.

Historical work in [v0.4-final](https://github.com/timfewi/tentaflake/tree/v0.4-final)
and [earlier development](https://github.com/timfewi/tentaflake/tree/ad28cc0e890df7b6653bfe99e03749237613fdd5)
is reference material, not a dependency or delivery commitment. Reuse only
pieces that solve a current requirement without importing obsolete authority.
See [archive comparison](archive-comparison.md). Issue status alone proves
neither runtime acceptance nor released support.
