# Secure installed-host quickstart

Installed hosts default to fail-closed `balanced`. This quickstart establishes
a stopped declaration, not vendor-runtime acceptance.

1. **Secure management first.** Review the
   [private management policy](11-tailscale-management.md) and replace identities
   in [the policy example](tailscale-policy.example.json). Install restricted
   network/SSH grants before relying on the tailnet alone. No allow-all policy,
   Funnel or public agent API; protect identity and recovery access.
2. **Select your installation's source and declare a stopped agent.** Use your
   [consumer/fork-owned TOML](08-agent-cli.md#installation-owned-configuration),
   not the public input's empty template. Track it in the owning Git repository
   before evaluation; missing or untracked desired state is an error. Follow
   [configuration](08-agent-cli.md) for manual editing or the installer-layout
   `agent plan|import` workflow, with schema-v1 TOML and adapter selection. Names are globally
   unique; use `agent-<name>` for all host services. Reviewed generic OCI images
   and in-capsule argv are supported; arbitrary host commands are not.
   OpenClaw/Pi are refusal scaffolds, not containers.
3. **Review storage before activation.** Default state is
   `/var/lib/tentaflake/agents/<name>`; effective workspace/owner come from
   adapter metadata. `autoStart = false` does not stop activation from creating
   declared quota filesystems or worker services. Existing data requires the
   explicit [quota migration](14-workspace-quota.md), never automatic hiding or
   conversion. Existing deployments also need [identity migration](agent-adapters.md#breaking-migration).
4. **Evaluate and build without activation.** From the resolved flake directory,
   build `.#nixosConfigurations.<configurationName>.config.system.build.toplevel` with
   `nix build --no-link`. Review assertions and changes. A closure is not live
   host evidence. [Build guidance](17-builds.md) describes the separate gates.
   The selector is the bootstrap flake attribute (`tentaflake.configurationName`),
   not the mutable TOML `[system].hostName`. Consumer flakes must set it to their
   exported attribute, as shown in `examples/consumer-flake.nix`.
5. **Activate deliberately.** Only after target/build review and operator
   authorization, use `sudo nixos-rebuild switch --flake <directory>#<configurationName>`.
   Import, evaluation and builds never imply permission to activate.
6. **Inspect the active host.** Run `tentaflake status` and `tentaflake doctor
   --security`; inspect the exact inventory unit, such as
   `docker-agent-assistant.service` (Podman uses `podman-`). Verify non-root,
   gVisor, read-only root, cap-drop-all, no-new-privileges, hardened tmpfs and
   resource bounds. Missing evidence stays unknown, not green.

Keep agents stopped until the exact ID has its enabled [LLM broker](12-brokered-egress.md),
[worker](13-disposable-worker.md), [workspace quota](14-workspace-quota.md) and
[Research relay](16-research.md). Balanced automatic start requires all four;
a declaration still does not prove native model/tool routing. The
[secure consumer example](../examples/adapter-secure.nix) combines these policies.
Generic model/Research integration is not accepted.

Never put provider keys, host networking, exposed ports or literal secrets in
a balanced capsule. Real keys remain in host runtime files. `dev` is an explicit
trusted compatibility profile, not an untrusted 24/7 boundary; `strict` fails
closed. Read [security profiles](10-security-profiles.md),
[secrets](04-agenix-secrets.md) and [operations](07-operations.md) before enabling
external access or planning recovery.
