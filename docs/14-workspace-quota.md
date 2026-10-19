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
and workspace behavior. It bounds sessions and caches written to the adapter's
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

Enabling the option and activating the resulting NixOS generation creates:

```text
/var/lib/tentaflake-workspace-volumes/
  hermes-coding.img
  state/hermes-coding.img
```

The prepare unit creates the exact-size sparse file, formats it as Btrfs, and
mounts it with `loop,nodev,nosuid,noatime`. This is an intentional filesystem
mutation. A source build or evaluation does not perform it and does not
authorize activation.

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
It also rejects a symlink/non-regular
backing path, a leftover `.new` file during image creation, or any size mismatch. It never mounts
over existing data, shrinks, grows, or reformats an existing image silently.
Existing ext4 images are rejected without modification and require the explicit
backup/restore migration below; this update does not convert them in place.

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
