#!/usr/bin/env python3
"""Offline regressions for local archive identity; no Docker or image acquisition."""

import contextlib
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import describe_docker_archive as archive


ARCHIVE = "/nix/store/00000000000000000000000000000000-offline-image.tar.gz"


def digest(value):
    return "sha256:" + hashlib.sha256(value).hexdigest()


def tar(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.PAX_FORMAT) as target:
        for name, data in entries:
            member = name if isinstance(name, tarfile.TarInfo) else tarfile.TarInfo(name)
            member.size = len(data)
            target.addfile(member, io.BytesIO(data))
    return output.getvalue()


def image(*, platform="amd64", config=None, layer=None, config_name=None):
    layer = tar([("hello", b"offline fixture\n")]) if layer is None else layer
    config = {
        "os": "linux",
        "architecture": platform,
        "rootfs": {"type": "layers", "diff_ids": [digest(layer)]},
    } if config is None else config
    raw = json.dumps(config, separators=(",", ":")).encode() if isinstance(config, dict) else config
    name = config_name or digest(raw).removeprefix("sha256:") + ".json"
    manifest = [{"Config": name, "RepoTags": ["local-fixture:test"], "Layers": ["layer/layer.tar"]}]
    return [
        ("layer/layer.tar", layer),
        (name, raw),
        ("manifest.json", json.dumps(manifest).encode()),
    ]


def describe(entries, compressed=False):
    data = tar(entries)
    if compressed:
        data = gzip.compress(data, mtime=0)
    return archive.describe(io.BytesIO(data), ARCHIVE)


class ArchiveTests(unittest.TestCase):
    def test_real_build_image_gnu_tar_root_marker(self):
        entries = image()
        with tempfile.TemporaryDirectory(prefix="tentaflake-archive-test-") as root:
            for name, data in entries:
                path = Path(root, name)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            # Same outer-archive producer/transform as pinned dockerTools.buildImage.
            result = subprocess.run(
                ["tar", "-C", root, "--hard-dereference", "--sort=name",
                 "--mtime=@1", "--owner=0", "--group=0", "--xform", "s:^./::", "-c", "."],
                check=True, capture_output=True, timeout=5,
                env={"PATH": os.environ["PATH"], "LC_ALL": "C"},
            )
        marker = tarfile.TarInfo.frombuf(result.stdout[:512], "utf-8", "strict")
        self.assertEqual((marker.name, marker.type, marker.size), (".", tarfile.DIRTYPE, 0))
        self.assertEqual(result.stdout[:100], b"./" + bytes(98))
        self.assertEqual(archive.describe(io.BytesIO(result.stdout), ARCHIVE), describe(entries))

    def test_root_marker_exception_does_not_accept_aliases_payloads_or_duplicates(self):
        root = tarfile.TarInfo(".")
        root.type = tarfile.DIRTYPE
        entries = image()
        with self.assertRaisesRegex(ValueError, "duplicate archive root directory"):
            describe([(root, b"")] + entries + [(root, b"")])
        with self.assertRaisesRegex(ValueError, "invalid archive root directory"):
            describe([(root, b"payload")] + entries)
        for name in ("", "/", "//", "././", "../"):
            alias = tarfile.TarInfo(name)
            alias.type = tarfile.DIRTYPE
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "unsafe archive member"):
                describe([(alias, b"")] + entries)
        for kind in (tarfile.REGTYPE, tarfile.SYMTYPE, tarfile.LNKTYPE):
            alias = tarfile.TarInfo("./")
            alias.type = kind
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                describe([(alias, b"")] + entries)

    def test_exact_descriptor_from_plain_and_gzip_archives(self):
        for platform in ("amd64", "arm64"):
            entries = image(platform=platform)
            expected = {
                "schemaVersion": 1,
                "format": "docker-archive",
                "archive": ARCHIVE,
                "imageId": digest(entries[1][1]),
                "os": "linux",
                "architecture": platform,
            }
            for compressed in (False, True):
                with self.subTest(platform=platform, compressed=compressed):
                    self.assertEqual(describe(entries, compressed), expected)

    def test_manifest_order_and_mutable_tags_do_not_select_identity(self):
        entries = image(config_name="config.json")
        expected = describe(entries)
        manifest = json.loads(entries[2][1])
        manifest[0]["RepoTags"] = ["other-name:latest"]
        entries[2] = ("manifest.json", json.dumps(manifest).encode())
        self.assertEqual(describe(list(reversed(entries))), expected)

    def test_timestamp_pax_and_directory_members(self):
        entries = image()
        member = tarfile.TarInfo(entries[0][0])
        member.mtime = 1.25
        directory = tarfile.TarInfo("layer/")
        directory.type = tarfile.DIRTYPE
        self.assertEqual(
            describe([(directory, b""), (member, entries[0][1])] + entries[1:]),
            describe(entries),
        )

    def test_layer_digests_use_actual_bytes_in_manifest_order(self):
        entries = image()
        entries[0] = (entries[0][0], tar([("hello", b"changed")]))
        with self.assertRaisesRegex(ValueError, "layer digest mismatch"):
            describe(entries)

        first = tar([("first", b"one")])
        second = tar([("second", b"two")])
        config = {
            "os": "linux",
            "architecture": "amd64",
            "rootfs": {"type": "layers", "diff_ids": [digest(first), digest(second)]},
        }
        entries = image(config=config, layer=first)
        manifest = json.loads(entries[2][1])
        manifest[0]["Layers"].append("second/layer.tar")
        members = entries[:2] + [("second/layer.tar", second)]
        self.assertEqual(
            describe(members + [("manifest.json", json.dumps(manifest).encode())])["imageId"],
            digest(entries[1][1]),
        )
        manifest[0]["Layers"].reverse()
        with self.assertRaisesRegex(ValueError, "layer digest mismatch"):
            describe(members + [("manifest.json", json.dumps(manifest).encode())])

    def test_config_digest_filename_is_checked(self):
        for name in ("0" * 64 + ".json", "blobs/sha256/" + "0" * 64):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "config filename digest"):
                describe(image(config_name=name))

    def test_exactly_one_image_and_all_referenced_members(self):
        entries = image()
        for count in (0, 2):
            manifest = json.loads(entries[2][1]) * count
            with self.subTest(count=count), self.assertRaisesRegex(ValueError, "exactly one image"):
                describe(entries[:2] + [("manifest.json", json.dumps(manifest).encode())])
        for missing in range(3):
            with self.subTest(missing=missing), self.assertRaisesRegex(ValueError, "missing image"):
                describe(entries[:missing] + entries[missing + 1:])

    def test_metadata_shape_platform_and_duplicate_keys(self):
        configs = [
            b'{"os":"linux","os":"linux"}',
            b'{"payload":"private fixture","broken":}',
            b'{"invalid":NaN}',
            [],
            {"os": "windows", "architecture": "amd64"},
            {"os": "linux", "architecture": "unsupported"},
            {"os": "linux", "architecture": "amd64", "rootfs": {}},
            {"os": "linux", "architecture": "amd64", "rootfs": {"type": "layers", "diff_ids": []}},
            {"os": "linux", "architecture": "amd64", "rootfs": {"type": "layers", "diff_ids": ["bad"]}},
        ]
        for config in configs:
            raw = json.dumps(config).encode() if isinstance(config, list) else config
            with self.subTest(config=config), self.assertRaises(ValueError) as error:
                describe(image(config=raw))
            self.assertNotIn("private fixture", str(error.exception))

    def test_unsafe_duplicate_and_conflicting_member_names(self):
        entries = image()
        for name in ("/absolute", "../escape", "a/../escape", "a//b", "./alias", "bad\\name"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "unsafe archive member"):
                describe(entries + [(name, b"x")])
        with self.assertRaisesRegex(ValueError, "duplicate archive member"):
            describe(entries + [entries[0]])
        with self.assertRaisesRegex(ValueError, "nondirectory ancestor"):
            describe(entries + [("layer", b"file")])

    def test_links_devices_and_sparse_outer_members_are_refused(self):
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE, tarfile.CHRTYPE, tarfile.GNUTYPE_SPARSE):
            member = tarfile.TarInfo("unsafe")
            member.type = kind
            member.linkname = "target"
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, "unsupported archive member type"):
                describe(image() + [(member, b"")])

    def test_pax_authority_overrides_are_refused(self):
        for field, value in (("path", "other"), ("size", "1"), ("GNU.sparse.map", "0,1")):
            member = tarfile.TarInfo("extra")
            member.pax_headers = {field: value}
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "unsupported archive PAX"):
                describe(image() + [(member, b"x")])

    def test_truncation_checksum_and_trailing_content(self):
        data = tar(image())
        bad_checksum = bytearray(data)
        bad_checksum[0] ^= 1
        for broken in (data[:100], data[:1000], bytes(bad_checksum), data + b"extra archive"):
            with self.subTest(length=len(broken)), self.assertRaises(ValueError):
                archive.describe(io.BytesIO(broken), ARCHIVE)
        with self.assertRaises((EOFError, OSError)):
            archive.describe(io.BytesIO(gzip.compress(data)[:-8]), ARCHIVE)

    def test_resource_limits_refuse_before_unbounded_metadata_reads(self):
        for bound, value in (
            ("MAX_HEADERS", 1),
            ("MAX_MEMBER_BYTES", 512),
            ("MAX_TOTAL_BYTES", 512),
            ("MAX_METADATA_BYTES", 32),
            ("MAX_CAPTURED_BYTES", 32),
            ("MAX_LAYERS", 0),
        ):
            with self.subTest(bound=bound), patch.object(archive, bound, value):
                with self.assertRaisesRegex(ValueError, "limit"):
                    describe(image())

    def test_oversized_headers_are_rejected_before_reading_missing_bodies(self):
        for kind, size, reason in (
            (tarfile.XHDTYPE, archive.MAX_PAX_BYTES + 1, "PAX limit"),
            (tarfile.REGTYPE, archive.MAX_MEMBER_BYTES + 1, "member byte limit"),
        ):
            member = tarfile.TarInfo("oversized")
            member.type = kind
            member.size = size
            header = member.tobuf(format=tarfile.GNU_FORMAT)
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, reason):
                archive.describe(io.BytesIO(header), ARCHIVE)

    def test_large_layers_are_read_in_bounded_chunks(self):
        class BoundedInput(io.BytesIO):
            calls = 0

            def readinto(self, buffer):
                self.calls += 1
                if len(buffer) > archive.CHUNK_BYTES:
                    raise AssertionError("unbounded archive read")
                return super().readinto(buffer)

        entries = image(layer=tar([("large", b"x" * (2 * archive.CHUNK_BYTES))]))
        source = BoundedInput(tar(entries))
        self.assertEqual(archive.describe(source, ARCHIVE)["imageId"], digest(entries[1][1]))
        self.assertGreater(source.calls, 2)

    def test_store_identity_is_explicit_and_cli_errors_do_not_echo_arguments(self):
        for name in ("/tmp/image", ARCHIVE + "/../other", ARCHIVE + "/", ARCHIVE + "\n"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "immutable Nix store path"):
                archive.describe(io.BytesIO(tar(image())), name)
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(archive.main(["/tmp/private fixture"]), 2)
        self.assertNotIn("private fixture", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
