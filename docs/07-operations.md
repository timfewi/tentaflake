# Operations and recovery

## Evidence boundaries

Nix evaluation proves option types and assertions. A build proves the closure.
Activation changes the host, and live checks prove only the activated runtime.
Do not treat one boundary as proof of another. Tentaflake never activates a
configuration merely because a check or build succeeded.

## Daily commands

```text
tentaflake status
tentaflake health
tentaflake doctor
tentaflake doctor --security
tentaflake logs <agent>
tentaflake stop <agent>
tentaflake start <agent>
tentaflake restart <agent>
```

Under `balanced`, Docker daemon access is root-equivalent and is not granted
through the `docker` group. Container commands use the CLI's sudo-backed path.
Podman containers are still root-managed by the NixOS OCI module unless a
separate rootless design is implemented; selecting Podman alone is not proof
of rootless operation.

## Safe update flow

1. Review input and image changes. A digest pin is reproducible identity, not
   evidence that the image is trustworthy. If the upstream signs releases,
   update and review its exact `tentaflake.imageProvenance` identity/key policy
   too; a mismatched signature gate blocks the controller before start.
2. Run formatting, lint, unit, evaluation, and VM checks.
3. Build the selected system without activation.
4. Review the exact target and build result.
5. Activate only in an approved maintenance window.
6. Run live status, security posture, service, and application health checks.

The previous NixOS generation remains the rollback path. Select it in the boot
menu or run the explicit rollback command from an operator session after
resolving the target. Source changes and successful builds do not authorize
`switch` or rollback.

## Git auto-push

The agent commits inside its workspace; a host unit performs the push with a
credential the balanced agent never receives:

```nix
gitAutoPush = {
  tokenEnvFile = "/run/agenix/github-coding";
  allowedRemotes = [
    "https://github.com/example/coding.git"
  ];
  allowedBranches = [ "main" ];
  interval = "2min";
};
```

The helper requires an exact canonical HTTPS GitHub repository and exact
branch. It rejects altered remotes and emits `ALLOW`, `DENY`, or `ERROR` to
journald. Use a fine-grained repository token or GitHub App credential with
only the required contents-write permission. The token file stays on the host.

## Legacy egress option

`tentaflake.networking.egress` is renamed to
`tentaflake.networking.legacyPortEgress` and is allowed only in `dev`:

```nix
tentaflake.networking.legacyPortEgress = {
  enable = true;
  allowedTCPPorts = [ 443 ];
};
```

This is a host `OUTPUT` destination-port filter. It does not constrain Docker
bridge `FORWARD`, identify destination hosts, control DNS, or constitute a
secure allowlist. Balanced agents use `network=none` or the dedicated broker
path documented in [brokered egress](12-brokered-egress.md). That path adds an
internal network, subnet-scoped INPUT/FORWARD policy, disabled container DNS,
and host-side destination resolution.

## Crash and reboot behavior

The upstream NixOS OCI module generates systemd units with
`Restart=on-failure`. Secure controllers and brokers share a recovery policy:
the restart delay increases exponentially from 10 seconds to one minute over
five steps, then stays at one minute. The start-limit window is disabled so a
prolonged transient failure does not permanently disable a 24/7 service.
Each attempt still runs the configured credential, mount, provenance, and
isolation gates. Persistent errors remain failed attempts; alert on restart
flapping and investigate their cause. An explicit systemd stop prevents
automatic restarts; declarative `autoStart` controls boot startup.

Brokers expose credential/policy-aware `/healthz`; this is not a provider
end-to-end probe. Backoff does not detect a hung or unhealthy process that
keeps running, or restart a controller after a failed dependency start job.
Use live health checks and the optional observability profile for those cases.

Hosts with a known, tested watchdog device can also opt in to PID 1 hardware
watchdog supervision:

```nix
tentaflake.hardening.watchdog = {
  enable = true;
  device = "/dev/watchdog";
  runtimeSec = "30s";
  rebootSec = "10min";
};
```

This is disabled by default because the template cannot know whether a target
has a functional watchdog or whether its firmware reset behavior is safe.
Validate the exact device and a controlled reset/recovery drill on the target
before relying on it.

## Backup and restore

Back up declarative configuration separately from mutable state. Per-agent
state defaults to `/var/lib/<runtime>-<name>`. Provider/Git/backup credentials
must remain outside the agent and outside backup logs.

The core module provides an opt-in Restic policy while keeping repository and
password values in runtime files owned by the deployment fork:

```nix
tentaflake.backup = {
  enable = true;
  paths = [
    "/var/lib/hermes-coding"
    "/var/lib/tentaflake-broker-llm-hermes-coding"
  ];
  repositoryFile =
    "/run/agenix/restic-repository";
  passwordFile =
    "/run/agenix/restic-password";
  pruneOpts = [
    "--keep-daily 7"
    "--keep-weekly 4"
  ];
};
```

Enabled managed quota workspaces below a selected path are automatically
added as separate Restic sources. This preserves `--one-file-system` without
silently skipping the mounted workspace. Only explicitly selected trees are
covered; unrelated agents and disabled quotas are excluded. The backup unit
requires the selected filesystems and asserts that each included quota
workspace is mounted before it runs. A failed mount prevents a new backup and
success timestamp. Other nested filesystems require their own explicit entry
in `paths`.

The job encrypts through Restic, prunes after backup, and runs an integrity
check. On success, a separate hardened oneshot updates
`/var/lib/tentaflake-backup/last-success` in a systemd-managed state directory;
the security doctor compares that timestamp with `lastSuccessMaxAgeHours` (36
by default). Its default timer is persistent, scheduled around 03:00 with
randomized delay. Alert on failed/stale `restic-backups-tentaflake.service`
runs.
For a restore drill: install a fresh test host, keep the agent stopped, restore
its state, verify ownership and file permissions, start the exact unit, and run
application-level checks. The VM test restores ordinary state and a mounted
quota workspace into a fresh directory, rejects an unavailable mount, and
verifies recovery afterward. Production backend credentials, retention,
capacity, and a real fresh-host drill remain operator evidence. Backup freshness and restore
readiness beyond freshness still requires an actual restore drill.

## Incident response and kill switch

1. Resolve and stop the exact agent with `tentaflake stop <agent>`.
2. Revoke its Git/provider/broker credentials outside the agent.
3. Preserve journald, optional Loki/Falco evidence, and a read-only state copy.
4. Inspect declared image, mounts, policy, workspace changes, and host events.
5. Build a repaired configuration and review it before activation.
6. Restore only reviewed state and rotate every possibly exposed credential.

`tentaflake stop <agent>` asks systemd to stop the container and every loaded
LLM/fetch broker for that exact container in one transaction. The internal
network remains declared but has no allowed service endpoint, so the virtual
key is inert. Provider/Git credential revocation remains an external operator
step.

Pending worker actions are separate private state. Inspect, approve, or deny
them with the commands in
[the disposable-worker guide](13-disposable-worker.md). Stopping an agent does
not authorize a pending action and an approval never grants network or secret
access to the worker.

## Disk and logs

The secure capsule limits RAM, swap, CPU, PIDs, ulimits, and tmpfs size. The
optional fixed-size workspace filesystem provides a hard per-controller code
workspace ceiling; see [workspace quota](14-workspace-quota.md). State outside
that mount and sparse backing images still require host free-space monitoring.
journald rotation follows the host configuration; the
optional Alloy/Loki profile is loopback-only and does not itself define a
retention policy suitable for every deployment. It does provide baseline
Prometheus rules for disk pressure, restart flapping, broker denials, unusual
fetch-denial bursts, and near-exhausted request/token/cost budgets. Alert
delivery remains an explicit deployment-fork integration.


## Continuous-runtime follow-ups (2026-10-03)

The source review for a modular 24/7 host identified these remaining issue
candidates. Restart backoff, broker health, quota-aware backups and optional
metrics already exist; the gaps below should not be described as missing all
supervision or backup support.

| Priority / proposed issue | Current limitation | Acceptance criteria |
| --- | --- | --- |
| P1: Application readiness and bounded hang recovery | `on-failure` recovers crashes, but a running hung agent remains running. Broker `/healthz` is not an end-to-end agent or Research probe. | Add adapter-defined, non-billable readiness/liveness checks with bounded timeouts, cooldown and explicit restart authority. Exercise hung processes, dependency recovery and operator stop without replaying operations. |
| P1: Actual vendor-agent acceptance | The Research integration VM uses a synthetic probe, not the pinned Hermes/ZeroClaw applications. Worker automatic routing and state survival also need real runtime evidence. | Run each reviewed image through model, Research discovery, isolated execution, restart/reboot and persistence scenarios with synthetic upstreams. Keep unsupported adapters stopped. See `16-research.md` and `agent-adapters.md`. |
| P1: Worker queue crash recovery and capacity | The selective archive review already identifies atomic claims, abandoned-work recovery and bounded pending/inbox capacity as follow-ups. | Implement queue lifecycle and negative crash/overload tests together, define migration, and retain quota-aware backup/restore. See `archive-comparison.md`. |
| P2: Research/management metrics and tested alert delivery | Optional observability covers host and broker metrics; notification destinations are deployment-owned. No source proof of end-to-end paging or shared Research/VPN readiness metrics was found. | Export categorical readiness/staleness/renewal/restart and usage metrics without queries, credentials or source text. Test a configured generic notification route in a deployment fixture. |

Private management and Headscale issue candidates are recorded in
[the management review](11-tailscale-management.md#modular-management-review-2026-10-03).
These candidates are prepared locally; no GitHub issues have been published.
