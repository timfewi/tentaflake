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

`tentaflake doctor --security` bounds each live OCI and broker-network
inspection to five seconds. A timeout is reported as unavailable/unknown
runtime evidence, never as a green posture. Broker health probes run in ordered
batches of at most 16; live OCI and broker-network inspection remains serial.
Large fleets should schedule the command through the operator's monitoring
path and size the run window for the number of agents.

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

The previous NixOS generation remains the code/configuration rollback path,
but a generation switch does not migrate mutable data. In particular, the
fixed-size worker-state image is a separate mount: an older generation can see
the directory below that mount rather than the queue/results stored in the
image. Before a rollback that crosses this storage-layout change, stop the
controller, worker, watcher, and cleanup units; reconcile pending and inflight
outcomes; archive reviewed results/audit data; then follow the offline rollback
steps in [the worker guide](13-disposable-worker.md). Select the previous
generation only after that data plan is explicit. Source changes and successful
builds do not authorize `switch` or rollback.

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
`Restart=on-failure`. Tentaflake adds a 10-second restart delay and bounded
start-limit window to secure controllers. Brokers use their own 5-second
delay/start limit and expose credential/policy-aware `/healthz`; this is not a
provider end-to-end probe. Declarative `autoStart` controls boot startup.

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

Worker state is not added to `tentaflake.backup.paths` automatically.
Restic uses filesystem-boundary protection, and copying the live sparse
`.img` backing file is neither an application-consistent queue backup nor a
safe replay plan. For a worker-state archive, first stop the exact controller,
worker, path watcher, cleanup timer, and cleanup service; verify all five
units are inactive, reconcile every `pending/` and `inflight/` request, then
back up the mounted, reviewed `results/` and
`audit.jsonl` paths explicitly. Do not restore `inflight/`, `jobs/`,
`worker.lock`, or `inbox.cursor` blindly. Restoring pending requests also
requires an operator decision about possible prior side effects and fresh job
IDs where an outcome is unknown.

The job encrypts through Restic, prunes after backup, and runs an integrity
check. On success, a separate hardened oneshot updates
`/var/lib/tentaflake-backup/last-success` in a systemd-managed state directory;
the security doctor compares that timestamp with `lastSuccessMaxAgeHours` (36
by default). Its default timer is persistent, scheduled around 03:00 with
randomized delay. Alert on failed/stale `restic-backups-tentaflake.service`
runs.
For a restore drill: install a fresh test host, keep the agent stopped, restore
its state, verify ownership and file permissions, start the exact unit, and run
application-level checks. The VM test backs up and restores a fixture into a
fresh target, but production backend credentials, retention, capacity, and a
real fresh-host drill remain operator evidence. Backup freshness and restore
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
