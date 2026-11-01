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

Host-side Git auto-push has been removed; `gitAutoPush` is not an accepted
adapter argument. No publication service, timer or credential is generated.

The former root service ran Git against agent-owned repository metadata with a
host token. Git hooks, included configuration and helpers can execute programs
with that authority. Checking the fetch URL also does not constrain `pushurl`,
URL rewrites or credential-helper destinations. Disabling hooks alone is not
a fix.

**Existing hosts require operator action; updating source alone does not stop
an installed service.** For each previously enabled agent:

1. Stop the timer and service immediately (substitute the configured agent name):
   `sudo systemctl stop hermes-NAME-autopush.timer hermes-NAME-autopush.service`.
2. Revoke the old publication credential and review repository/account audit
   logs for unexpected access. Rotate any other credentials reachable by the
   former root service if compromise is suspected.
3. Remove `gitAutoPush`, then rebuild and activate the host
   configuration through your normal operator workflow. Confirm the old timer
   and service are no longer installed or running.
4. Remove the unused token-file provisioning if no other integration needs it.

Local agent commits and `gitIdentity` remain supported. There is no automatic
publication replacement. Publish only through an independently reviewed,
least-privilege workflow with trusted Git metadata and an exact operator-owned
repository/branch grant. Do not run host Git in the agent workspace or copy its
`.git` directory into a credentialed checkout. Review exported content as
untrusted input; neither a token kept outside the container nor a remote
allowlist alone isolates Git execution.

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
state defaults to `/var/lib/tentaflake/agents/<name>`. Provider/Git/backup credentials
must remain outside the agent and outside backup logs.

The optional public `storage` section of `tentaflake.toml` projects directly to
the existing opt-in Restic module. Repository and password values stay in
operator-provisioned runtime files, never TOML or the Nix store:

```toml
[storage]
enable = true
paths = ["/var/lib/tentaflake/agents/coding", "/var/lib/tentaflake-broker-llm-agent-coding"]
repositoryFile = "/run/agenix/restic-repository"
passwordFile = "/run/agenix/restic-password"
```

The section defaults disabled and refuses incomplete enabled declarations or
disabled sections with additional fields. It requires 1–128 unique explicit
sources: canonical per-agent state/workspace and broker trees, or an exact
declared `stateDir` (including custom paths such as `/var/lib/private/kept`).
It does not discover other host state or accept arbitrary source paths.
Repository/password and optional `environmentFile` references are safe paths
strictly below `/run/`; their contents are read only at runtime. Provision the
repository and protect the files separately. No public initialization, reset,
restore, automatic relocation or executable policy is exposed. See the
[public contract](08-agent-cli.md#public-ordinary-backups) for exact bounds.

Existing consumer Nix backup configuration remains independent, including
reviewed sources outside that public allowlist and retention/timer policy:

```nix
tentaflake.backup = {
  enable = true;
  paths = [
    "/var/lib/tentaflake/agents/coding"
    "/var/lib/tentaflake-broker-llm-agent-coding"
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

Enabled managed quota state/workspace mounts below a selected path are automatically
added as separate Restic sources. This preserves `--one-file-system` without
silently skipping a mounted state or workspace filesystem. Only explicitly selected trees are
covered; unrelated agents and disabled quotas are excluded. The backup unit
requires the selected filesystems and asserts that each included quota
filesystem is mounted before it runs. A failed mount prevents a new backup and
success timestamp. Other nested filesystems require their own explicit entry
in `paths`.

The job encrypts through Restic, prunes after backup, and runs an integrity
check. On success, a separate hardened oneshot updates
`/var/lib/tentaflake-backup/last-success` in a systemd-managed state directory;
the security doctor compares that timestamp with `lastSuccessMaxAgeHours` (36
by default). Its default timer is persistent, scheduled around 03:00 with
randomized delay. Alert on failed/stale `restic-backups-tentaflake.service`
runs.
The ordinary Restic job and `tentaflake backup <name>` retain their existing
filesystem-backup behavior. Neither is a cross-service consistency checkpoint:
copying a live SQLite database without a coordinated writer boundary can omit
WAL state, and restoring an older coherent database can still forget spend or
dispatched operations. A successful backup timestamp is not recovery readiness.
The existing VM restore test covers selected mounts and filesystem recovery,
not end-to-end runtime/budget/worker reconciliation.

**Consistent production recovery is BLOCKED / NOT IMPLEMENTED.** The offline
`scripts/tentaflake-recovery.py` artifact is not registered in the backup module,
CLI, timer or production package. Its `checkpoint` command refuses unconditionally:
canonical writer coverage and actual runtime, broker, worker and control
lifecycle admission holds are not yet integrated. A whole-system checkpoint also
requires a supported checkpoint interface from the independent board service;
no board database or replacement task registry is inferred.

The helper's synthetic checkpoint core exercises stop-before-copy, private
partial evidence, complete SQLite/WAL files, required authority receipts and a
digest-bound manifest carrying scope, revision, runtime and schema identities.
It rejects missing receipts and running worker claims. Its offline `restore`
command requires an externally retained manifest digest and a **new, absent
target beneath a private operator-owned directory**; an already existing empty
directory is deliberately refused. It copies into numbered private directories,
preserves original ownership/modes as manifest evidence rather than granting
service ownership, and writes a quarantine hold before copying state. It does
not provision credentials, overwrite live paths, activate services, replay work,
clear holds, or reconcile/refund budget counters. Interrupted restores remain
private and quarantined, without a completed restore receipt.

These markers are evidence, **not yet an operational lifecycle enforcement
mechanism**. Do not promote these artifacts into live state or start services
against them. Even a coherent old snapshot needs explicit operator comparison
against current provider spend, operation IDs, worker dispatch/results and
board/runtime identities; clearing broker unsettled reservations alone does not
prove that an older snapshot's counters are safe. Production reconciliation and
admission integration, backend credentials, retention/capacity and a fresh-host
restore drill remain required operator evidence. Focused synthetic coverage lives
in `scripts/test_backup_recovery.py`; it does not establish production readiness.

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
fixed-size workspace filesystem provides a hard per-controller code workspace
ceiling; optional private state uses a separate bound. See
[workspace/state quotas](14-workspace-quota.md). Other host state and aggregate
sparse backing images still require host free-space monitoring.
journald rotation follows the host configuration; the
optional Alloy/Loki profile is loopback-only and does not itself define a
retention policy suitable for every deployment. It does provide baseline
Prometheus rules for disk pressure, restart flapping, broker denials, unusual
fetch-denial bursts, and near-exhausted request/token/cost budgets. Alert
delivery remains an explicit deployment-fork integration.


## Continuous-runtime follow-ups (2026-10-03)

Crash recovery is not [application hang/readiness acceptance](https://github.com/timfewi/tentaflake/issues/112).
[Real vendor acceptance](https://github.com/timfewi/tentaflake/issues/111) and
[tested alert delivery](https://github.com/timfewi/tentaflake/issues/114) remain
separate from source fixtures. Follow current [worker recovery](13-disposable-worker.md),
not historical queue proposals. Management/Headscale evidence limits are in
[the management review](11-tailscale-management.md#modular-management-review-2026-10-03).
