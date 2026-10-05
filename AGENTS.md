# Agent Instructions — tentaflake

NixOS flake template for running isolated AI agents on a single machine.
See `docs/agent-adapters.md` for current support. The planned common runtime and
simple installed-host onboarding in `docs/roadmap.md` precede employee/company
features; planned capability admission does not relax current startup gates.

## Build & Test

`just` recipes load the pinned contributor tools automatically; `just list`
lists them. Enter `nix develop` before running Cargo or lint tools directly.
Start with `just fast`; use affected VM suites for runtime/security changes.
GitHub check selection imports `.github/ci-paths.json`; docs skip Nix/Research
builds, and CI metadata uses routing regressions rather than VMs. Runtime and
Research changes select affected suites; unknown paths/history require the full
gate. Keep the real-Git regression check passing.

```bash
nix flake check
nix build .#installer-iso
nix build \
  .#checks.x86_64-linux.vm-integration \
  -L
nix build --no-link .#checks.x86_64-linux.research-policy
nix build --no-link -L .#checks.x86_64-linux.research-integration
cargo fmt --all -- --check
cargo clippy --workspace \
  --all-targets -- -D warnings
cargo test --workspace
just fast
just e2e
just e2e-devcontainer
just e2e-installer
just e2e-run-vm
just security
```

New host installs and managed workspace images use Btrfs. Workspace images
require at least 128 MiB; never automatically reformat or convert existing ext4
images. Follow `docs/14-workspace-quota.md` for explicit migration.
Installer disk operations live in `installer/disk.sh`, imported by the wizard
and VM fixture. Keep cleanup within the selected disk stack; refuse cross-disk
VGs before mutation and preserve unrelated swap, VGs and encrypted mappings.

## Conventions

- Nix: `nix fmt` (nixfmt), 2-space indent
- Rust: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- Commits: Conventional Commits (`feat:`, `fix:`, `docs:`)
- DCO: every non-merge PR commit needs `Signed-off-by:`; use `git commit -s`
  with the human user's configured Git author and committer identity. Never
  replace that identity with an agent identity.

## Template Rule

This repo is a GENERIC template. NEVER commit domain-specific code (company config, real hostnames,
hardware configs, API keys, secrets, agent SOUL.md/skills written for specific deployments).
Domain-specific work belongs in FORKS, not here.

## Keep Docs In Sync

After any change that alters behavior, options, or usage, verify the docs are
still accurate before finishing — and update them in the same change:

- `README.md` and `docs/` — user-facing docs
- this `AGENTS.md` / `CLAUDE.md` — agent instructions
- relevant `.agents/skills/` — bundled skill docs

Keep skills short: actionable steps, essential boundaries and verification.
Link detailed option tables/examples to their owning guides; verify native
Hermes commands against the pinned image rather than copying upstream catalogs.
Never print credentials as a diagnostic check.

The public repository owns the documentation; the website presents a pinned
release. See `docs/18-documentation.md`. Contributors update authoritative
guides here and identify route additions/removals in the PR. Website publication
is a separate maintainer operation; never imply that unreleased checkout changes
are already published or require access to another repository for public checks.

## Module Boundaries

- Editor, Hive Research, and Piper integrations belong in consumer flakes;
  the template no longer exports these modules or Piper voice assets.
- Tailscale SSH/hostname preferences use `extraSetFlags`; `--advertise-tags`
  belongs to `extraUpFlags` and manual `tailscale up` enrollment. NixOS applies
  `extraUpFlags` only with `authKeyFile`. Keep management access separate from
  Research VPN readiness and never infer remote grants from a local enable flag.
- `modules/` — reusable NixOS modules, including security, brokered egress,
  image-provenance gates, disposable workers, workspace quotas, encrypted
  backup, and generic options
- `lib/` — helpers (`mkHermesAgent`, `mkZeroClawAgent`, `agentsFromData`, `pinnedImage`, `constants`, `devshell`)
- `adapters/catalog.json` owns versioned preset/capability/evidence facts for Nix,
  configuration-free `tentaflake runtimes`, and generated adapter docs. Run
  `python3 scripts/runtime-catalog-docs.py` after catalog changes. Generic
  definitions use `lib/runtimeContract.nix` and the shared `containerSecurity`
  builder; no model/Research acceptance or startup-gate relaxation is implied.
- `adapters/` — lazy runtime registry and schema-v1 contracts; `lib/mkAgent.nix`
  selects one adapter. Legacy wrappers preserve their APIs and state. OpenClaw
  is a stopped scaffold without an OCI artifact; never advertise operational
  support before real model/Research/execution/lifecycle acceptance.
- Instance inventory uses explicit `tentaflake.agentInstances` metadata for
  adapters, retaining unmanaged OCI fallback. Generic JSON requires
  `schemaVersion = 1`; generic and legacy arrays are additive with collisions
  rejected. See `docs/agent-adapters.md` and `checks.*.agent-adapters`.
- `crates/` and `pkgs/` — Rust CLI/broker/worker workspace and Nix packages
- CLI help is configuration-free; management commands require generated
  host configuration and inventory.
  `agent template` is also configuration-free. Onboarding validates through
  `lib/agentPlan.nix` and the shared JSON parser; imports add stopped entries
  atomically without activation. Preserve ordinary flake source filtering and
  never print raw Nix diagnostics containing operator source excerpts.
- Host diagnostics share checked systemd/disk evidence; failed or unknown
  agent states are problems, while stopped agents remain valid. `--hide`
  must redact host and agent names in both text and JSON.
- `lib/mkRustPackage.nix` shares Rust packaging: root Cargo files, all member
  manifests, and only the selected crate's source. Add required root inputs
  explicitly. Versions come from the root Cargo workspace; descriptions from
  crate manifests. Derive worker image references from the image metadata.
  Shared container identities live in `lib/constants.nix`.
- A balanced agent with `autoStart = true` requires its exact LLM broker, worker,
  workspace-quota, and research relay declarations; stopped scaffolds remain `network=none`.
- Brokers retry with `RestartMode=direct` so temporary failures preserve the
  controller process. Explicit broker stops must still stop `Requires` controllers.
  Direct retries skip systemd failure hooks; verify health/restart evidence in a VM.
- Web/research uses only the pinned `tentaflake-research` stdio MCP server
  `secure-research-tool`; model calls use the LLM broker. Do not add a second
  web transport, re-enable legacy fetch, expose other agent sockets, or enable
  provider-hosted network tools. See `docs/16-research.md` and its evidence limits.
  Research client recovery must never replay dispatched operations or reopen
  after explicit close; every new session repeats negotiation and UID checks.
- Security manifests carry exact live mount, tmpfs, network, and resource
  expectations. Incomplete/legacy runtime evidence must remain unknown.
  Secure bind sources must not be nested below another writable source. Generic
  state/workspace sources use an unmounted root-owned parent; refuse symlinks
  or unsafe parent ownership/permissions before container startup.
  OCI inspection has a deadline and a combined output-byte ceiling; failures
  remain unknown and raw container environment data must not reach reports.
- Broker host concurrency/rate admission counts each enabled LLM/fetch mode.
  LLM SSE is opt-in through `llm.streaming.enable`; retain bounded event/response
  sizes, first/idle/total deadlines, cancellation, no request replay and
  conservative reservation. Audit terminal admission before forwarding success;
  partial-output failure must close without fabricating completion. Streaming
  fixtures do not establish OpenClaw runtime acceptance.
  Preserve only optional bounded `x-opencode-session` affinity metadata on both
  LLM routes and transports; never treat it as authority, log it, or forward
  arbitrary client headers. The client owns session propagation.
  Clear systemd capability sets with an empty string; an empty list omits the
  directive. Verify credential write boundaries inside the unit namespace.
- Secure controllers and brokers share `lib/serviceRecovery.nix`: bounded
  restart backoff without permanent start-limit exhaustion. Explicit stops
  must still prevent automatic retries.
- `tests/` — NixOS VM test backing `checks.<system>.vm-integration`
- Worker queue mutations share the private per-agent lock. Sync atomic request
  publication and pending-to-running claims before OCI dispatch; never replay
  abandoned running state. Recovery requires exact container ownership and
  retains unknown outcomes. Bound aggregate inbox and pending/running capacity;
  keep approval and existing-state migration semantics in `docs/13-disposable-worker.md`.
- Restic backups retain filesystem boundaries; selected managed quota mounts
  are separate sources and required mounts. Verify workspace restore as well
  as ordinary state when changing backup or quota behavior.
  Optional `workspaceQuota.agents.<container>.state` must match adapter
  `stateStorage` and ownership. Initialize adapter directories only after the
  state mount; seeds/healing wait for quota ownership. Refuse symlink sources
  and parents before changing permissions. Never hide/migrate existing state.
- `installer/` — installer ISO and disk-install scripts
- `examples/` — consumer-flake reference
- `docs/` — user-facing documentation
- `.agents/skills/` — bundled Hermes skills (also development agent instructions for this repo)

## PR Description

Fill out the full template at `.github/PULL_REQUEST_TEMPLATE.md` — a title alone
is not enough. Write a real `## Description` (what changed and why), check
the applicable `Type of Change` box, and tick every checklist item that
applies (leave inapplicable ones unchecked, don't delete them).

See CONTRIBUTING.md for PR process.
