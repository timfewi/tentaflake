# Persistent workspace and state quotas

## Result

`tentaflake.workspaceQuota.agents.<container>` mounts a fixed-size Btrfs
filesystem at one controller workspace, and optionally a separate one at its
private state source. Each backing file has an exact declared
size of at least 128 MiB, so both controller writes and worker-inbox writes receive a real
filesystem `ENOSPC` instead of consuming the host root filesystem without a
per-agent ceiling.

A deliberately stopped balanced scaffold may omit this disk mutation. An
automatically started balanced controller must declare its matching quota;
evaluation fails until the path and numeric owner match the runtime builder.

Private state is opt-in through `state`; omitting it preserves existing state
and workspace behavior for legacy adapters. Generic workloads require both
workspace and private-state quotas before automatic or manual startup; stopped
definitions may be planned without either. It bounds sessions and caches written to the adapter's
state mount. Broker/worker audit, backups, other host paths and aggregate sparse
image allocation still require host free-space monitoring. This change does not
relax startup admission or establish acceptance of a new agent runtime.

## Declaration

The key, path, and owner must match the runtime builder:

```nix
tentaflake.workspaceQuota.agents.hermes-coding = {
  enable = true;
  workspace =
    "/var/lib/hermes-coding/workspace";
  sizeMiB = 8192;
  ownerUid = 10000;
  ownerGid = 10000;
  state = {
    path = "/var/lib/hermes-coding";
    sizeMiB = 1024;
  };
};
```

ZeroClaw uses numeric UID/GID 65534 by default. Builder assertions
reject a path or owner mismatch. The agent unit requires the mount/ownership
unit, so a missing image, failed filesystem check, mount failure, or ownership
failure prevents the controller from starting.

`state.path` must exactly match the adapter's declared `stateStorage`, with the
same numeric ownership as the agent. Hermes and ZeroClaw use their existing
state directory; generic workloads use `/var/lib/generic-NAME/state` beneath
the unmounted root-owned instance parent. State sizes default to 1024 MiB and
must be at least 128 MiB. Paths must be normalized, outside backing images,
and outside the workspace. Mounts may not overlap another agent's filesystems.

## First activation is a disk mutation

With `security.admission = null`, enabling the quota option and activating the
resulting NixOS generation creates:

```text
/var/lib/tentaflake-workspace-volumes/
  hermes-coding.img
  state/hermes-coding.img
```

The prepare unit selects immutable create or verify helpers; a separate mount
unit mounts the image with `loop,nodev,nosuid,noatime`, followed by an immutable
ownership helper. Creation exclusively opens a `.new` staging file, sets its
exact sparse size and formats Btrfs. It syncs the file, publishes without
overwriting an existing final path, and syncs the directory before and after
removing staging. Interrupted staging remains for explicit recovery. Verification
is read-only and refuses a missing image; it never falls back to creation.
These legacy units call helpers directly. An explicit host admission declaration
selects the guarded lifecycle below and refuses creation until bootstrap.
Activation is an intentional filesystem mutation; a source build or evaluation
does not perform it or authorize activation.

The managed mount starts after the host's ordinary local filesystems and
tmpfiles setup. Its ownership unit then recreates the private worker-control
directories inside the mounted filesystem before the controller or worker
watcher may start. The inbox is writable only by the exact agent UID/GID and
the host worker's statically declared matching service group. Unsafe symlink
or non-directory control paths fail closed. Private state ownership is a
dependency of workspace ownership, so its failure or explicit stop also stops
dependent controllers. For legacy layouts with a workspace below state, the
state mount and directory initialization complete before workspace preparation.
Seeds and UID healing wait for these ownership units.

The module checks each mount source and its parents for symlinks/non-directories
before changing permissions or preparing an image. State initialization also
checks adapter-owned child directories before creating or changing them.
The module refuses to mount over non-empty state/workspace sources, including
files written below an unmounted path while its backing image already exists.
It tolerates only the empty `.tentaflake-worker/inbox` scaffolding that NixOS
tmpfiles may create during workspace activation; private state must be empty.
Backing-image directories and their ancestors must be root-owned and not
group/world writable. Creation and verification reject symlink/non-regular
backing paths, size mismatch and leftover `.new` entries, including staging
beside an existing final image. They never mount over existing data, shrink,
grow, or reformat an existing image silently.
Existing ext4 images are rejected without modification and require the explicit
backup/restore migration below; this update does not convert them in place.

## Read-only admission inventory

The optional [host admission plan](10-security-profiles.md#shared-admission-core)
also supports a read-only diagnostic. As root, pass
its resolved immutable store file; the `/etc` symlink itself is not accepted:

```bash
tentaflake admission inventory "$(readlink -f /etc/tentaflake/admission.json)" --hide
```

The command validates the closed plan, owner/resource/binding matches and exact
unit names, then scans `/var/lib/tentaflake-workspace-volumes` and its optional
`state` subdirectory. Protected descriptor-relative traversal refuses symlinks,
unsafe ownership/permissions, non-regular or hardlinked images, unfinished
`.new` files and incomplete enumeration. An absent root is an error, not an
empty inventory. The bounded scan includes removed definitions and orphan
images, using positive logical file sizes rather than allocated sparse blocks.
Only exact declared slot/size matches retain a binding; size drift stays orphaned.

Every returned allocation is `Unknown`, and command output always has
`complete: false`, including an empty existing root. This diagnostic does not
fence creators, initialize a ledger, verify Btrfs/mount readiness or establish
physical free capacity. It cannot authorize image creation or ledger bootstrap.
`--hide` redacts allocation and binding identities.

## Guarded storage lifecycle

Setting `security.admission` replaces the prepare/mount/owner chain with the
existing quota ownership service as a single supervised transaction. Legacy
prepare and native mount units are masked. Controllers, seeds and backups keep
their quota-service dependencies; Restic additionally verifies the exact mounted
image before reading it. The supervisor mounts in the host namespace with
`loop,nodev,nosuid,noatime`; state starts before workspace and stops after it.

First bootstrap is an explicit maintenance operation. Stop all Docker workloads
and quota services, detach all loop devices, and load the generated
guarded units before running as root:

```bash
tentaflake admission bootstrap "$(readlink -f /etc/tentaflake/admission.json)"
```

Bootstrap refuses an existing or partial `/var/lib/tentaflake-admission` ledger,
queued or surviving legacy quota work, unmasked file-backed native mount units,
unavailable Docker observations, unsafe roots and unfinished staging. It checks
loaded and installed mount units, including removed definitions. Refusing all
attached loops avoids mistaking a backing-image alias for an unrelated image.
It creates only missing
protected backing directories, imports all images including removed definitions,
and records verified exact declared Btrfs images as Present. Other images remain
Unknown and charged. It never stops foreign workloads or converts images.
Do not delete a ledger or change its authority epoch to retry bootstrap.

Start the generated quota service through systemd. Internal `volume-start` and
`volume-stop` commands require its exact current invocation, executable, command
arguments, PID, cgroup, cleanup policy and host mount namespace. An exclusive
protected host lock serializes transactions and is inherited by trusted helpers;
systemd bounds their whole control group to 300 seconds at start and 120 at stop.
A busy lock refuses the operation after five seconds; retry the service after
the existing transaction finishes. Direct CLI calls cannot create an image.

Creation requires a new synced Unknown ledger allocation under the published
immutable plan. Only successful create/mount/ownership completion marks it
Present. Restart verifies existing Present images; missing, partial, mismatched
or Unknown images never fall back to creation. Stopping unmounts the exact image
and retains its full charge. Exact cleanup is allowed under its original loaded
unit even when a changed declaration prevents new starts.

Before new creation, all retained logical bytes, the new image size and
`physicalSpaceHeadroomBytes` must fit the current available bytes of the shared
backing filesystem. Root, state directory and images must share its device and
mount identity. Missing and orphan images remain charged; allocated sparse or
shared blocks receive no credit. This conservative guard can refuse early and
does not reserve extents against unrelated host writes or guarantee that later
filesystem metadata allocation cannot exhaust space.

After a failed transaction, stop all quota services and Docker workloads and
detach all loop devices before read-only recovery of an exact retained slot:

```bash
tentaflake admission storage-recover "$(readlink -f /etc/tentaflake/admission.json)" workspace:hermes-coding
```

Recovery only records a verified existing image as Present. It never recreates,
resizes, deletes, rebinds or releases storage. Preserve partial files for explicit
repair. Changing the admission manifest currently blocks new starts until host
publication orchestration is implemented; never reset existing authority as a
workaround. This storage path does not yet account for aggregate OCI compute or
establish model, Research, strict-kernel or vendor acceptance.

## Existing workspace migration

The Restic backup module includes enabled quota mounts inside its selected
`paths` as separate sources and requires them to be mounted before backup.
Selecting the parent agent-state directory covers both state and its quota
workspace; generic instance parents also include their separate state mount.
Only mounts within selected trees are included. Other nested filesystems need
explicit backup paths. Restore files
into the mounted state/workspace with the agent stopped; do not overwrite a live
backing image. A live file backup does not provide application-level snapshot
consistency.

Perform this only in an approved maintenance window and adapt paths to the
exact stopped agent:

1. Back up and verify the existing workspace.
2. Stop the exact agent and its brokers.
3. Move the old workspace to a reviewed temporary path on the same host.
4. Activate the generation with the quota declaration; confirm the empty
   filesystem mounted and has the configured byte size.
5. Copy reviewed data into the mounted workspace without crossing its limit.
6. Verify numeric ownership, run the security doctor, and start the agent.
7. Keep the verified backup until a restore/start drill passes.

Tentaflake intentionally provides no automatic migration command because a
wrong source, destination, or size would be destructive.

## Adding a private state quota to an existing agent

Use the same explicit backup/restore procedure for `state.path`. Stop the
controller, worker, seeds and any other state writers first. For a workspace
already mounted below that state path, unmount it before moving the old state
directory. Preserve both its backing image and old state data. Do not copy a
mounted child filesystem or recreate worker scaffolding in the empty state
mount point. After activation, verify state mounted first and the existing
workspace image mounted beneath it; restore only reviewed state files, excluding
the separately backed-up workspace. Verify ownership, state and workspace
contents, both limits and a restore/start drill before removing originals.
No automatic migration or host activation is provided.

## Migrating existing ext4 quota images

Stop the controller and worker, back up the mounted workspace files, and verify
that backup. Unmount the workspace and preserve the exact old backing image at
a separate reviewed path. Declare at least 128 MiB, then activate the Btrfs
configuration to create a new empty image. Restore the files into that mounted
workspace and verify ownership, restored contents, the size ceiling, and agent
startup before discarding any original image or backup. Copy files, not an ext4
image, into the new Btrfs workspace. No automated in-place conversion is provided.

## Resize and recovery

Changing `sizeMiB` while the image exists fails closed. Resize requires an
explicit backup/restore into a new Btrfs image of the declared size, with the
controller and worker stopped and the old image preserved until verification.
The module never grows or shrinks a live image automatically.

At boot, the prepare service checks the filesystem type and runs
`btrfs check --readonly` only while the expected image is not mounted at its
target. A failed check prevents the mount and therefore prevents controller startup. It performs no
automatic repair; preserve the image and restore from a verified backup rather
than running `btrfs check --repair` as an automatic recovery step.

`ReadWritePaths` creates a private bind mount in the service namespace even
when the quota is absent. Both preparation and ownership identify the exact
loop backing image; a mount-point or filesystem-type check alone is insufficient.
Ownership refuses an absent or mismatched image and verifies its configured
byte size before modifying directories. Changing a size also changes the
ownership unit, so an active mount cannot retain stale size admission.

## Verification boundary

Module tests prove declaration matching, mount dependencies and fixed path/options.
The adapter check executes generated quota helpers against temporary symlink,
non-empty state, incomplete/unsafe image, size and filesystem-format fixtures;
unrelated permissions and contents must stay unchanged. The VM test boots state
and workspace mounts, verifies the
exact backing-file size and Btrfs type, and confirms a write beyond the limit
fails. It also checks reboot persistence, backup/restore across the mount, and
rejection of legacy ext4 images with checksums unchanged. It also exercises
unit stop propagation and state-directory symlink refusal inside the unit
namespace. Without a
completed VM/live run, evaluation alone is not quota proof.
