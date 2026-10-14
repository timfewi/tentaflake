#!/usr/bin/env python3
"""Describe one local Docker archive; never extract, load, pull, or verify a publisher.

Accept plain/gzip archives with regular files, directories and timestamp-only
PAX headers, as emitted by dockerTools. Other extensions fail closed. Bounds:
1024 headers, 128 layers, 16 GiB/member, 64 GiB decoded total, 1 MiB/JSON and
16 MiB retained small-member data. Layer bytes are hashed in 1 MiB chunks.
"""

import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import sys
import tarfile
import zlib


MAX_HEADERS = 1024
MAX_LAYERS = 128
MAX_MEMBER_BYTES = 16 * 1024**3
MAX_TOTAL_BYTES = 64 * 1024**3
MAX_METADATA_BYTES = 1024**2
MAX_CAPTURED_BYTES = 16 * MAX_METADATA_BYTES
MAX_PAX_BYTES = 4096
MAX_TRAILER_BYTES = 1024**2
CHUNK_BYTES = 1024**2
STORE_PATH = re.compile(
    r"/nix/store/[0-9abcdfghijklmnpqrsvwxyz]{32}-[A-Za-z0-9+._?=-]+"
    r"(?:/[A-Za-z0-9+._?=-]+)*"
)
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def store_path(value):
    return (
        isinstance(value, str)
        and len(value) <= 4096
        and STORE_PATH.fullmatch(value) is not None
        and all(part not in (".", "..") for part in value.split("/"))
    )


def member_name(value):
    require(
        isinstance(value, str)
        and 0 < len(value) <= 255
        and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+/-]*", value) is not None
        and all(part not in ("", ".", "..") for part in value.split("/")),
        "unsafe archive member name",
    )
    return value


def decode(raw):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "duplicate image metadata key")
            result[key] = value
        return result

    try:
        def invalid_constant(_):
            raise ValueError("invalid image metadata JSON constant")

        return json.loads(raw, object_pairs_hook=unique, parse_constant=invalid_constant)
    except (UnicodeError, json.JSONDecodeError, RecursionError):
        raise ValueError("invalid image metadata JSON") from None


class Reader:
    def __init__(self, source):
        self.source = source
        self.total = 0

    def read(self, size):
        require(0 <= size <= CHUNK_BYTES, "archive read limit")
        value = self.source.read(size)
        self.total += len(value)
        require(self.total <= MAX_TOTAL_BYTES, "decoded archive byte limit")
        return value

    def exact(self, size):
        pieces = []
        remaining = size
        while remaining:
            value = self.read(remaining)
            require(value, "truncated archive")
            pieces.append(value)
            remaining -= len(value)
        return b"".join(pieces)


def timestamp_pax(raw):
    """Accept only inert timestamps; reject path, size, sparse and link overrides."""
    position = 0
    keys = set()
    while position < len(raw):
        space = raw.find(b" ", position)
        require(space > position, "invalid archive PAX record")
        length_text = raw[position:space]
        require(len(length_text) <= 5 and length_text.isdigit(), "invalid archive PAX length")
        end = position + int(length_text)
        require(space + 1 < end <= len(raw) and raw[end - 1:end] == b"\n", "invalid archive PAX record")
        key, separator, value = raw[space + 1:end - 1].partition(b"=")
        require(
            separator
            and key in (b"mtime", b"atime", b"ctime")
            and key not in keys
            and len(value) <= 64
            and re.fullmatch(rb"-?[0-9]+(?:\.[0-9]+)?", value) is not None,
            "unsupported archive PAX metadata",
        )
        keys.add(key)
        position = end


def read_members(source):
    # Parse headers with the standard TAR checksum/parser, but handle extensions
    # before reading their payload: TarFile otherwise allocates PAX/sparse data
    # before a caller can apply its member limits.
    reader = Reader(source)
    members = {}
    captured = 0
    pending_pax = False
    root_seen = False
    for _ in range(MAX_HEADERS):
        header = reader.exact(512)
        if header == bytes(512):
            require(not pending_pax, "dangling archive PAX header")
            require(reader.exact(512) == bytes(512), "invalid archive terminator")
            trailing = 0
            while value := reader.read(CHUNK_BYTES):
                trailing += len(value)
                require(trailing <= MAX_TRAILER_BYTES and not any(value), "unexpected archive trailer")
            return members
        try:
            member = tarfile.TarInfo.frombuf(header, "utf-8", "strict")
        except (tarfile.HeaderError, UnicodeError, ValueError):
            raise ValueError("invalid archive header") from None
        require(0 <= member.size <= MAX_MEMBER_BYTES, "archive member byte limit")
        if member.type in (tarfile.XHDTYPE, tarfile.XGLTYPE):
            require(not pending_pax and member.size <= MAX_PAX_BYTES, "archive PAX limit")
            timestamp_pax(reader.exact(member.size))
            pending_pax = member.type == tarfile.XHDTYPE
        else:
            require(member.type in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE), "unsupported archive member type")
            pending_pax = False
            # GNU tar's transformed `-c .` (dockerTools.buildImage) emits './'.
            # Check raw fields: TarInfo strips directory trailing slashes, which
            # must not widen this one inert exception to alternate root names.
            if member.isdir() and header[:100] == b"./" + bytes(98) and header[345:500] == bytes(155):
                require(member.size == 0 and not member.linkname, "invalid archive root directory")
                require(not root_seen, "duplicate archive root directory")
                root_seen = True
                continue
            name = member_name(member.name.removesuffix("/") if member.isdir() else member.name)
            require(name not in members, "duplicate archive member")
            if member.isdir():
                require(member.size == 0, "nonempty archive directory")
                members[name] = None
            else:
                # DockerTools puts config/manifest JSON after its layers. Keep
                # bounded candidate metadata, never the known layer TAR bodies.
                keep = member.size <= MAX_METADATA_BYTES and not name.endswith(".tar")
                if keep:
                    captured += member.size
                    require(captured <= MAX_CAPTURED_BYTES, "archive metadata retention limit")
                digest = hashlib.sha256()
                data = bytearray() if keep else None
                remaining = member.size
                while remaining:
                    value = reader.exact(min(remaining, CHUNK_BYTES))
                    digest.update(value)
                    if keep:
                        data.extend(value)
                    remaining -= len(value)
                members[name] = ("sha256:" + digest.hexdigest(), data)
        padding = (-member.size) % 512
        require(not any(reader.exact(padding)), "nonzero archive member padding")
    raise ValueError("archive header limit")


def metadata(members, name):
    member_name(name)
    require(name in members and members[name] is not None, "missing image metadata member")
    digest, raw = members[name]
    require(raw is not None and len(raw) > 0, "image metadata byte limit")
    return digest, decode(raw)


def describe(source, archive):
    require(store_path(archive), "archive identity must be an immutable Nix store path")
    buffered = io.BufferedReader(source)
    stream = gzip.GzipFile(fileobj=buffered, mode="rb") if buffered.peek(2)[:2] == b"\x1f\x8b" else buffered
    members = read_members(stream)
    for name in members:
        for parent in Path(name).parents:
            if str(parent) != "." and str(parent) in members:
                require(members[str(parent)] is None, "archive member has a nondirectory ancestor")
    _, manifest = metadata(members, "manifest.json")
    require(isinstance(manifest, list) and len(manifest) == 1, "archive must contain exactly one image")
    image = manifest[0]
    require(isinstance(image, dict) and isinstance(image.get("Config"), str), "invalid image manifest")
    layers = image.get("Layers")
    require(isinstance(layers, list) and len(layers) <= MAX_LAYERS, "image layer count limit")
    image_id, config = metadata(members, image["Config"])
    basename = image["Config"].rsplit("/", 1)[-1]
    if re.fullmatch(r"[0-9a-fA-F]{64}\.json", basename):
        require(basename == image_id.removeprefix("sha256:") + ".json", "image config filename digest mismatch")
    if image["Config"].startswith("blobs/sha256/"):
        require(image["Config"] == "blobs/sha256/" + image_id.removeprefix("sha256:"), "image config filename digest mismatch")
    require(isinstance(config, dict), "invalid image config")
    require(config.get("os") == "linux" and config.get("architecture") in ("amd64", "arm64"), "unsupported image platform")
    rootfs = config.get("rootfs")
    require(isinstance(rootfs, dict) and rootfs.get("type") == "layers", "invalid image root filesystem")
    diff_ids = rootfs.get("diff_ids")
    require(isinstance(diff_ids, list) and len(diff_ids) == len(layers), "image layer identity count mismatch")
    for name, expected in zip(layers, diff_ids):
        member_name(name)
        require(isinstance(expected, str) and DIGEST.fullmatch(expected) is not None, "invalid image layer identity")
        require(name in members and members[name] is not None, "missing image layer")
        require(members[name][0] == expected, "image layer digest mismatch")
    return {
        "schemaVersion": 1,
        "format": "docker-archive",
        "archive": archive,
        "imageId": image_id,
        "os": config["os"],
        "architecture": config["architecture"],
    }


def open_archive(path):
    flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK
    parent = os.open("/nix/store", flags | os.O_DIRECTORY)
    try:
        parts = Path(path).parts[3:]
        for part in parts[:-1]:
            child = os.open(part, flags | os.O_DIRECTORY, dir_fd=parent)
            os.close(parent)
            parent = child
        return os.open(parts[-1], flags, dir_fd=parent)
    finally:
        os.close(parent)


def main(argv):
    if len(argv) != 1 or not store_path(argv[0]):
        print("usage: describe_docker_archive.py /nix/store/<archive>", file=sys.stderr)
        return 2
    try:
        fd = open_archive(argv[0])
        with os.fdopen(fd, "rb") as source:
            info = os.fstat(source.fileno())
            require(stat.S_ISREG(info.st_mode) and info.st_mode & 0o222 == 0 and 0 < info.st_size <= MAX_TOTAL_BYTES, "archive must be a bounded read-only regular file")
            result = describe(source, argv[0])
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0
    except ValueError as error:
        print("local image descriptor: " + str(error), file=sys.stderr)
    except (OSError, EOFError, tarfile.TarError, RecursionError, zlib.error):
        print("local image descriptor: unreadable or invalid archive", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
