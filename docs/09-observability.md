# Observability and runtime detection

Tentaflake exposes two independent profiles. Neither is imported by the core.

## Scoped workload resource identity

`modules/workload-mapping.nix` is an opt-in, fixed root observer, separate from
the host-wide dashboards below. `tentaflake.workloadMapping.enable = true`
enables it; `bindings.<canonical-agent-id>` contains exactly `tenant_id`,
`workload_id`, and `policy_revision`. These bindings are host authorization
policy, not another instance registry: only runnable `agentInstances` backed
by the exact declared Docker container and unit are accepted. The helper
package is `pkgs/tentaflake-workloads`.

The first implementation supports **rootful Docker with systemd cgroups only**.
The exact kernel path must be `/system.slice/docker-<full-container-id>.scope`.
Podman, rootless/user-namespaced/remapped containers, custom cgroup parents,
hybrid cgroups, stopped scaffolds and ambiguous declarations are refused, not
guessed. Configure Docker's `native.cgroupdriver=systemd` explicitly when the
host default differs. The root operator, Docker daemon, immutable helper and
host policy are trusted. Docker socket access is inherently privileged; only
this fixed helper receives it, never Hostpulse or an agent. Its only engine
operation is a bounded inspect template returning ID, PID and running state;
there is no command API, environment inspection, log access or namespace entry.
`CAP_SYS_PTRACE` is retained solely to verify the target's initial user
namespace through procfs; no network-administration/BPF/mount capability is
granted. The observer has a private network and only Unix address-family
access.

After deployment, bootstrap **once on a genuinely new host**:

```console
systemctl stop tentaflake-workloads.service
systemctl start tentaflake-workloads-initialize.service
systemctl start tentaflake-workloads.service
```

Initialization is never a dependency of the normal service. It refuses an
existing authority file, including a corrupt one. Normal execution refuses
missing/corrupt authority. Preserve and back up
`/var/lib/tentaflake-workloads/authority.json` (root-only) without rolling its
generation counter back; loss requires explicit operator reconciliation, not
deleting state and rerunning bootstrap. An interrupted `.pending` publication
also fails closed and requires operator inspection/reconciliation. Do not
remove it while the observer is running. Direct helper modes are precisely
`--initialize`, `--run`, and `--once`; policy, state and runtime locations are
fixed, not command-line inputs.

The observer atomically publishes root-owned `0444`
`/run/tentaflake-workloads/<agent-id>.json` in a root-owned `0755` directory.
The closed version-1 schema contains `state`, `tenant_id`, `agent_id`,
`workload_id`, `policy_revision`, `unit`, `generation`, `verified_at_ms`,
`kernel_boot_id`, `container_id`, `pid`, `start_ticks`, `cgroup`, and
`cgroup_inode`. `cgroup` is the absolute unified-hierarchy path, not a path
prefixed with `/sys/fs/cgroup`. In `unavailable` state all six kernel/process
fields are null. No SDK run identity is asserted. Global monotonically
allocated generations advance on policy, boot, process, cgroup identity or
live/unavailable transitions; unchanged facts survive observer restarts.
Identity is checked before and after inspection against procfs and the
cgroup-v2 filesystem/inode. Observations repeat after a two-second sleep;
each engine/unit command has a one-second deadline and 4096-byte output cap.
A slow or denied observation is unavailable, never a zero resource value.

Hostpulse needs only metadata and kernel-counter read access, with independent
exact UID/tenant/agent reader grants. Consumers must enforce a maximum
10-second manifest lease, current policy scope, boot/inode/process identity
and before/after sampling checks; clear rates on identity change. Service
death or a blocked poll expires rather than retaining live authority.
Policy removal unlinks the prior manifest on restart; the Nix service restarts
when the generated policy changes. Runtime files are not an authorization
grant to read other tenants, and host-wide metadata is never a fallback.

`tests/workload-mapping-vm.nix` is the explicit isolated integration seam:
`{ pkgs, extraModules ? [], extraTestScript ? "" }`. It runs two local,
synthetic Docker workloads without external networking, within two CPUs and
2048 MiB; it compares manifests to kernel membership/inodes and exercises
stop/restart, read permissions and corrupt/missing authority. Parent
integration can add Hostpulse/UI configuration through `extraModules` and
counter comparison/cross-tenant assertions through `extraTestScript`, before
the final authority-corruption checks. This fixture is not native vendor/SDK
runtime attestation. The focused Rust command after workspace registration is
`cargo test -p tentaflake-workloads`; run the VM as a separate explicit gate,
not as part of fast checks.

## Observability profile

`nixosModules.observability` enables:

- Prometheus for time-series storage;
- the Prometheus node exporter for host metrics;
- Loki for journal logs;
- Alloy to forward the local systemd journal to Loki;
- Grafana with provisioned Prometheus and Loki data sources.
- local textfile metrics for configured broker denial/budget state and bundled
  Prometheus alert rules.

Every listener binds to `127.0.0.1`; the profile opens no firewall port.
Grafana signup, update checks, analytics, Gravatar, and feedback links are
disabled.

Enable it in a consumer or fork:

```nix
imports = [
  inputs.tentaflake.nixosModules.observability
];

tentaflake.profiles.observability = {
  enable = true;
  retention = "14d";
  grafanaSecretKeyFile =
    "/run/agenix/grafana-secret-key";
  grafanaAdminPasswordFile =
    "/run/agenix/grafana-admin-password";
};
```

The two required files are loaded through systemd credentials. Their contents
must be stable across restarts and must not enter the Nix store. Evaluation
fails when either path is missing from the configuration.

Default loopback ports:

| Service | Port |
|---|---:|
| Grafana | 3000 |
| Prometheus | 9090 |
| Node exporter | 9100 |
| Loki | 3100 |
| Alloy diagnostics | 12346 |

Publishing Grafana through a tailnet or reverse proxy is a deployment decision.
Require authentication and TLS as appropriate; do not expose the other
listeners merely to make Grafana reachable.

The local rules cover root-disk pressure, repeated agent/service state changes,
recent broker policy denials, a burst of rejected fetches, and request, token,
or cost budgets above 90 percent. A hardened root oneshot reads only the exact
configured broker audit/budget paths once per minute and writes numeric metrics
for node exporter's textfile collector; it does not export prompt bodies or
credentials. Missing state files produce zero usage until a broker has written
state.

Prometheus evaluates these rules, but this generic profile deliberately does
not guess an email, pager, webhook, or external Alertmanager destination.
Configure and test notification delivery in the deployment fork. Until that
is done, a firing rule is visible in Prometheus/Grafana but is not proof that an
operator was paged. Thresholds are a baseline and should be tuned from observed
normal traffic.

## Falco profile

`nixosModules.falco` is separate because kernel event capture is a materially
different trust boundary. It runs Falco directly under systemd with the modern
eBPF engine and the capabilities `CAP_BPF`, `CAP_PERFMON`, `CAP_SYS_RESOURCE`,
and `CAP_SYS_PTRACE`.

The pinned nixpkgs revision does not provide Falco. Supply a reviewed and pinned
package whose layout includes `bin/falco` and the upstream configuration tree:

```nix
imports = [
  inputs.tentaflake.nixosModules.falco
];

tentaflake.profiles.falco = {
  enable = true;
  package = myPinnedFalcoPackage;
};
```

Modern eBPF generally requires a kernel at least 5.8 with BTF and the BPF ring
buffer. Check the exact Falco release requirements before deployment. The
profile does not download drivers or silently run a privileged container.

Falco defaults to the package's configuration. Use `configFile` and
`extraArgs` only with reviewed rules and outputs. Runtime alerts are written to
Falco's configured output; integrate them with journald or another explicitly
chosen destination.

Evaluation proves option wiring and systemd configuration. It does not prove
kernel compatibility, event capture, rule quality, or alert delivery. Verify
those on the target host after an explicitly authorized activation.

Upstream references:

- <https://falco.org/docs/concepts/event-sources/kernel/>
- <https://falco.org/docs/setup/download/>
- <https://grafana.com/docs/alloy/latest/reference/components/loki/loki.source.journal/>
