# Persistent workspace quota and migration

## Result

`tentaflake.workspaceQuota.agents.<container>` mounts a fixed-size ext4
filesystem at one controller workspace. Its backing file has an exact declared
size, so both controller writes and worker-inbox writes receive a real
filesystem `ENOSPC` instead of consuming the host root filesystem without a
per-agent ceiling.

A deliberately stopped balanced scaffold may omit this disk mutation. An
automatically started balanced controller must declare its matching quota;
evaluation fails until the path and numeric owner match the runtime builder.

This module deliberately covers the code workspace, not every runtime-specific
state path. Sessions, caches, broker state, audit, backups, and the backing
image directory still require host free-space monitoring.

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
};
```

ZeroClaw uses numeric UID/GID 65534 by default. Builder assertions
reject a path or owner mismatch. The agent unit requires the mount/ownership
unit, so a missing image, failed filesystem check, mount failure, or ownership
failure prevents the controller from starting.

## First activation is a disk mutation

Enabling the option and activating the resulting NixOS generation creates:

```text
/var/lib/tentaflake-workspace-volumes/
  hermes-coding.img
```

The prepare unit fully preallocates the exact-size backing file, formats it as
ext4, and mounts it with `loop,nodev,nosuid,noatime`. It also verifies the
allocation before checking an existing unmounted image, so insufficient host
disk space fails during activation rather than later under agent load. This is
an intentional filesystem mutation. A source build or evaluation does not
perform it and does not authorize activation.

The managed mount starts after the host's ordinary local filesystems and
tmpfiles setup. The worker's path activation uses Linux inotify, so the managed
workspace must be local storage; do not place it on a remote NFS filesystem.
Its ownership unit then recreates the private worker-control
directories inside the mounted filesystem before the controller or worker
watcher may start. The inbox is writable only by the exact agent UID/GID and
the host worker's statically declared matching service group. Unsafe symlink
or non-directory control paths fail closed.

The module refuses first creation when the declared workspace contains data.
It tolerates only the empty `.tentaflake-worker/inbox` scaffolding that NixOS
tmpfiles may create during activation. It also rejects a symlink/non-regular
backing path, a leftover `.new` image, or any size mismatch. It never mounts
over existing data, shrinks, grows, or reformats an existing image silently.

## Existing workspace migration

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

## Resize and recovery

Changing `sizeMiB` while the image exists fails closed. Resize requires a
separate offline procedure: stop the controller/worker, unmount the exact
workspace, verify a backup, check the filesystem, grow the backing file, run
`resize2fs`, update the declaration, and rebuild. Shrinking is substantially
riskier and should use backup/restore into a new image instead.

At boot, the prepare service runs `e2fsck -p` only while the workspace is not
mounted. Return codes above the automatically repairable class fail the mount
and therefore fail the controller dependency. Preserve the image and restore
from backup rather than forcing a damaged filesystem online.

## Verification boundary

Module tests prove declaration matching, mount dependencies, fixed path/options,
and the fail-closed non-empty check. The VM test boots the mount, verifies the
exact backing-file size and ext4 type, and confirms a write beyond the limit
fails. Without a completed VM/live run, evaluation alone is not quota proof.
