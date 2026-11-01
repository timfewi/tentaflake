# This runs AFTER the OCI module's load ExecStartPre and BEFORE engine run.
# Host root/engine administrators remain trusted; no registry/pull fallback.
{
  lib,
  pkgs,
  backend,
  bundle,
  image,
}:
assert builtins.elem backend [
  "docker"
  "podman"
];
assert bundle.imageVerificationVersion == 1;
assert bundle.imageSelector == image;
let
  # Docker's containerd store can report a manifest digest as .Id (API v1.56
  # ImageSummary), not the classic config digest. Export is local-only; hash
  # the exact config bytes, never the tag, manifest, or reserialized JSON.
  # https://github.com/docker/docs/blob/main/content/manuals/build/metadata/annotations.md
  exportProof = pkgs.writeText "tentaflake-local-image-proof.py" ''
    import hashlib
    import json
    import os
    import re
    import resource
    import subprocess
    import sys
    import tarfile

    engine, selector, expected, directory = sys.argv[1:]
    archive = os.path.join(directory, "image.tar")

    def refuse():
        raise ValueError("invalid local image proof")

    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                refuse()
            result[key] = value
        return result

    def members():
        # Uncompressed streaming mode: no extraction, decompression bombs, or
        # seeking over an unchecked config. Bound metadata too (including PAX).
        with tarfile.open(archive, mode="r|") as tar:
            for count, member in enumerate(tar):
                if count >= 100000:
                    refuse()
                yield tar, member

    def bounded_member(tar, member, limit):
        if not member.isfile() or not 0 < member.size <= limit:
            refuse()
        with tar.extractfile(member) as stream:
            data = stream.read(limit + 1)
        if len(data) != member.size:
            refuse()
        return data

    try:
        # Hard disk bound applies to the export child as well. The shell owns
        # the private directory and cleanup even on timeout or parser failure.
        resource.setrlimit(resource.RLIMIT_FSIZE, (2 * 1024**3, 2 * 1024**3))
        with open(archive, "xb") as output:
            subprocess.run(
                [engine, "image", "save", selector],
                stdout=output, stderr=subprocess.DEVNULL, check=True, timeout=150,
            )
        resource.setrlimit(resource.RLIMIT_AS, (256 * 1024**2, 256 * 1024**2))
        manifest = None
        seen_manifest = False
        for tar, member in members():
            if member.name == "manifest.json":
                if seen_manifest:
                    refuse()
                seen_manifest = True
                manifest = json.loads(
                    bounded_member(tar, member, 64 * 1024),
                    object_pairs_hook=unique_object,
                )
        if not isinstance(manifest, list) or len(manifest) != 1:
            refuse()
        entry = manifest[0]
        if not isinstance(entry, dict) or entry.get("RepoTags") != [selector]:
            refuse()
        config = entry.get("Config")
        if not isinstance(config, str) or not re.fullmatch(
            r"(?:[0-9a-f]{64}\.json|blobs/sha256/[0-9a-f]{64})", config
        ):
            refuse()
        digest = None
        for tar, member in members():
            if member.name == config:
                if digest is not None:
                    refuse()
                digest = "sha256:" + hashlib.sha256(
                    bounded_member(tar, member, 4 * 1024 * 1024)
                ).hexdigest()
        if digest != expected:
            refuse()
    except Exception:
        # Never expose archive/inspect payloads or engine diagnostics.
        sys.exit(1)
  '';
in
pkgs.writeShellScript "tentaflake-verify-local-image" ''
  set -euo pipefail
  expected="$(${pkgs.coreutils}/bin/cat ${lib.escapeShellArg "${bundle}/image-id"})"
  if [[ ! "$expected" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "tentaflake: invalid immutable image proof" >&2
    exit 1
  fi
  actual="$(
    ${pkgs.coreutils}/bin/timeout --kill-after=5s 20s \
      ${lib.getExe pkgs.${backend}} image inspect --format '{{.Id}}' ${lib.escapeShellArg image} \
      2>/dev/null | ${pkgs.coreutils}/bin/head -c 256
  )"
  if [[ "$actual" =~ ^[0-9a-f]{64}$ ]]; then actual="sha256:$actual"; fi
  if [[ "$actual" == "$expected" ]]; then exit 0; fi
  ${lib.optionalString (backend == "docker") ''
    if [[ "$actual" =~ ^sha256:[0-9a-f]{64}$ ]]; then
      umask 077
      proof_dir="$(${pkgs.coreutils}/bin/mktemp -d -- "''${TMPDIR:-/tmp}/tentaflake-image-proof.XXXXXXXXXX")"
      trap '${pkgs.coreutils}/bin/rm -rf -- "$proof_dir"' EXIT
      trap 'exit 1' HUP INT TERM
      if ${pkgs.coreutils}/bin/timeout --kill-after=5s 180s \
        ${pkgs.python3}/bin/python3 ${exportProof} \
        ${lib.escapeShellArg (lib.getExe pkgs.docker)} ${lib.escapeShellArg image} "$expected" "$proof_dir"
      then
        exit 0
      fi
    fi
  ''}
  echo "tentaflake: loaded local image does not match reviewed build output" >&2
  exit 1
''
