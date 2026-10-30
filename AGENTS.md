# Agent instructions — Tentaflake

Generic NixOS template for isolated agents. Deployment identities, hardware,
secrets, private memories and company policy belong in forks. Nix/TOML/settings
and seed inputs enter the Nix store. Never print credentials for diagnostics.
Do not replace declarative agents with a mandatory runtime, UI or control service.

## Ownership

- `agents.toml` is the only public instance-definition source. Public
  `agentsFromData { file = ./agents.toml; }` is file-only; `mkAgent` and adapter
  builds are internal. Each `adapters/<id>.nix` owns one runtime implementation
  and pure `preset` facts. `lib/runtimeCatalog.nix`
  projects presets; `adapters/catalog.json` and the marked adapter-doc table are
  generated with `python3 scripts/runtime-catalog-docs.py`, never edited as
  independent runtime sources.
- `lib/runtimeContract.nix` owns generic definitions; `lib/containerSecurity.nix`
  owns shared containment. `modules/` owns host broker, Research, worker, quota,
  backup, provenance and management policy. Runtime hooks cannot weaken it;
  host policy may consume instance metadata but must not define more instances.
- All managed IDs are `agent-<name>`, names globally unique; default host state
  is `/var/lib/tentaflake/agents/<name>`. Read explicit `agentInstances` metadata
  for effective paths, `stateStorage`, workspace, numeric owners and `runnable`.
  Inventory retains unmanaged OCI fallback. TOML requires `schemaVersion = 1`
  and `agents`; optional `_securityNote` is metadata. No legacy wrapper/data APIs.
  Ship only the empty inventory (`schemaVersion = 1`, `agents = []`), without
  an example file. Remaining `agents.json` requires explicit migration and must
  never be silently ignored, including when TOML also exists.
- Hermes/ZeroClaw real vendor acceptance is pending. OpenClaw/Pi are stopped
  refusal units only: no OCI artifact, state, credentials or sockets. Generic
  custom OCI is offline, with no accepted model/Research integration. Synthetic
  fixtures are not vendor acceptance. See [adapters](docs/agent-adapters.md).
- `crates/` owns CLI/broker/worker. Shared Rust packaging in `lib/mkRustPackage.nix`
  includes workspace manifests and selected crate source; root versions and
  image metadata remain authoritative. Keep required root inputs explicit.
- CLI help/catalog/templates are configuration-free. Management uses generated
  host inputs; imports validate via the shared parser/plan, publish stopped
  additions atomically and never stage, activate or download runtime images.
  Preserve ordinary Git flake filtering and redact raw Nix source diagnostics.
- Editor, Hive Research and speech integrations belong in consumer flakes.

## Security and state contracts

- Balanced automatic startup requires the exact ID's LLM broker, worker,
  workspace quota and Research relay. `strict` fails closed; `dev` is explicitly
  trusted compatibility, not a fallback. Generic ownership/containment is not
  relaxed to admit another runtime.
- Keep real provider keys host-side, model calls on the broker and web solely
  on pinned `secure-research-tool`. Preserve exact read-only socket/closure
  mounts, no remote MCP/provider-hosted network tools and no operation replay.
  Research reconnection repeats protocol/UID checks; explicit close stays closed.
- VPN observer options belong to `services.secureResearch.vpnObserver`, disabled
  by default. Operators own exits/markers. Fresh leases and bounded drain retain
  the old firewall until completion/expiry, then require a new ready generation.
  Management enrollment is not Internet-exit evidence; see [Research](docs/16-research.md).
- Management capabilities describe configured transport/SSH, not enrollment or
  remote grants. Preserve the enable alias, unsupported-transport refusal and
  unknown legacy manifests. No public OpenSSH in balanced. Tailscale SSH options
  use `extraSetFlags`; advertised tags use enrollment `extraUpFlags`.
- Security manifests describe exact mounts, tmpfs, network and resource bounds.
  Unknown/incomplete live evidence remains unknown. OCI inspection is bounded
  by time/output and never exposes raw environments. Reject writable bind-source
  nesting; generic source parents stay unmounted, root-owned/private and
  symlink-checked before startup. Check source/parent symlinks before privileged
  permission changes.
- Broker admission is host-bounded. SSE is opt-in with bounded bytes/events,
  deadlines, cancellation, conservative reservations and no replay. Audit
  terminal admission before success; never fabricate completion after partial
  output. Only bounded optional `x-opencode-session` affinity is forwarded,
  never logged or treated as authority. Clear systemd capabilities with an
  empty string, not an omitted empty-list directive.
- Strict budgets are exclusive, checksum-protected durable accounting: no
  refunds/reset on uncertain outcomes. Initialization/reconciliation are
  operator-only one-shots, never automatic. Preserve settled admission-day spend
  and held reservations. Broker dynamic identities cannot collide with host users.
  See [broker policy](docs/12-brokered-egress.md).
- Worker mutations share one private lock. Durably publish requests and running
  claims before OCI dispatch; never replay abandoned/unknown work. Bound inbox
  and pending/running capacity; verify exact container ownership on recovery.
  Readable directory descriptors, not `O_PATH`, are needed for fsync. Approval
  never grants network/secrets/host authority or proves native tool mediation.
  See [worker policy](docs/13-disposable-worker.md).
- New host installs and managed quota images use Btrfs; images need at least
  128 MiB. No automatic ext4 conversion, state relocation, deletion or budget
  reset. Quotas match metadata paths/owners; initialize after mounting, with
  seeds/healing behind ownership units. Identify exact loop backing images,
  not merely a filesystem/mountpoint (`ReadWritePaths` adds bind mounts).
  Back up selected state/workspace mounts separately and verify both restores.
- Shared recovery uses bounded backoff without permanent start-limit exhaustion.
  Broker direct retries preserve controllers; explicit stops must still stop
  dependencies and prevent retries. A running process is not application health.
- Host Git must never run on agent-owned repository metadata with publication
  credentials. Removed publication services require explicit operator shutdown
  and credential revocation, not a replacement auto-push implementation.
- Installer disk mutations stay in `installer/disk.sh`; constrain cleanup to
  the selected disk stack and refuse cross-disk VGs before mutation. Preserve
  unrelated swap, VGs and encrypted mappings.

## Optional host API

`host-api/` and `modules/host-api.nix` are Unix-only, disabled by default;
actions/logs/worker decisions have separate grants. Use canonical inventory IDs,
root peer verification and private API group membership, never arbitrary unit,
command, socket, credential or Nix activation inputs. Durable receipts precede
dispatch; uncertain retries retain request IDs and never automatically replay.
Policy details are declarations; typed/revision-bound configuration proposals
only return escaped draft Nix. They never apply it. Host-only mode owns no task
state and claims no native chat. Worker decisions capture full argv/digest under
the queue lock, first decision wins; persist the drain marker before scheduling
and retain the level-triggered watcher. A saved decision is not execution
success. See [API contract](docs/19-web-ui.md).

## Checks and handoff

Read [CONTRIBUTING](CONTRIBUTING.md) and [build boundaries](docs/17-builds.md).
`just` loads pinned tools; direct Cargo/lint commands need `nix develop`.
Start with `just fast` (including pure-preset `catalog-check`); host configuration
evaluation, generated flakes, packages and VM/runtime acceptance are separate
explicit gates. CI routing uses `.github/ci-paths.json`; unknown paths/history
select all fast checks/full dispatch inventory. Skipped builds are not evidence.
Keep the sequential `scripts/eval_flake.py` projections and Git source filtering;
prepare exact Research closures without mounting the whole store. VM/system/ISO
workloads and activation need their own authorization. Read-only formatting is
`just fmt-check`; treefmt `--ci` can rewrite files.

Synchronize affected guides/examples/shared skills in the same change. Keep
skills procedural and compact; detailed options belong to owning guides. The
public repo owns docs; the website mirrors a pinned release, not this checkout.
Report actual checks and evidence gaps. No build authorizes activation.
Use Conventional Commits and DCO sign-off with the human's configured identity;
preserve unrelated work. Complete every applicable PR-template section and
checklist. Report vulnerabilities through GitHub Security Advisories.
