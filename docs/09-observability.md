# Observability and runtime detection

Tentaflake exposes two independent profiles. Neither is imported by the core.

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
