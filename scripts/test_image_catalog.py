#!/usr/bin/env python3
"""Check reviewed base-image metadata offline; never fetch or inspect image layers."""

import base64
import copy
from datetime import datetime
import hashlib
import json
from pathlib import Path
import re
import unittest

from jsonschema import Draft202012Validator
from referencing import Registry
from referencing.exceptions import NoSuchResource


ROOT = Path(__file__).resolve().parents[1]
MAX_FILE_BYTES = 2 * 1024 * 1024
MAX_EVIDENCE_BYTES = 4 * 1024 * 1024
PLATFORMS = {
    "x86_64-linux": {"os": "linux", "architecture": "amd64"},
    "aarch64-linux": {"os": "linux", "architecture": "arm64", "variant": "v8"},
}
INDEX_TYPE = "application/vnd.oci.image.index.v1+json"
MANIFEST_TYPE = "application/vnd.oci.image.manifest.v1+json"
CONFIG_TYPE = "application/vnd.oci.image.config.v1+json"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def decode(raw):
    def unique(pairs):
        result = {}
        for name, value in pairs:
            require(name not in result, "duplicate metadata JSON key")
            result[name] = value
        return result

    return json.loads(raw, object_pairs_hook=unique)


def local_schema(schema):
    def walk(value):
        if isinstance(value, dict):
            require("$id" not in value and "$dynamicRef" not in value, "schema scope override")
            if "$ref" in value:
                require(value["$ref"].startswith("#/"), "nonlocal catalog schema reference")
            for child in value.values():
                walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)

    def reject_remote(uri):
        raise NoSuchResource(ref=uri)

    walk(schema)
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema, registry=Registry(retrieve=reject_remote))


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def checked_json(raw, expected_digest, expected_size=None):
    require(0 < len(raw) <= MAX_FILE_BYTES, "metadata file byte limit")
    require(digest(raw) == expected_digest, "raw metadata digest mismatch")
    if expected_size is not None:
        require(type(expected_size) is int and expected_size == len(raw), "descriptor size mismatch")
    return decode(raw)


def validate(catalog, schema, evidence):
    local_schema(schema).validate(catalog)
    require(sum(map(len, evidence.values())) <= MAX_EVIDENCE_BYTES, "aggregate evidence byte limit")
    expected_files = set()
    for identity, image in catalog["images"].items():
        require(identity == f"{image['distro']}-{image['release']}", "base image identity mismatch")
        require(image["repository"] == f"docker.io/library/{image['distro']}", "repository mismatch")
        require(image["discoveryTag"].startswith(image["codename"] + "-"), "discovery tag mismatch")
        observed = datetime.fromisoformat(image["observedAt"])
        require(observed.tzinfo is not None, "metadata observation needs a timezone")
        directory = f"{identity}/"
        index_raw = evidence[directory + "index.json"]
        index = checked_json(index_raw, image["indexDigest"])
        require(evidence[directory + "index-by-digest.json"] == index_raw, "immutable index roundtrip mismatch")
        expected_files.update((directory + "index.json", directory + "index-by-digest.json"))
        require(index["schemaVersion"] == 2 and index["mediaType"] == INDEX_TYPE, "unsupported image index")
        require(isinstance(index["manifests"], list) and len(index["manifests"]) <= 128, "index descriptor limit")
        for system, platform in image["platforms"].items():
            expected = PLATFORMS[system]
            require(platform["platform"] == expected, "catalog platform does not match Nix system")
            candidates = [entry for entry in index["manifests"] if entry.get("platform") == expected]
            require(len(candidates) == 1, "missing or ambiguous platform descriptor")
            descriptor = candidates[0]
            require(descriptor["digest"] == platform["manifestDigest"], "manifest not selected by index")
            require(descriptor["mediaType"] == MANIFEST_TYPE, "unsupported platform manifest")
            annotations = descriptor["annotations"]
            require(annotations["org.opencontainers.image.source"] == image["sourceUrl"], "source annotation mismatch")
            version = image["codename"] if image["distro"] == "debian" else image["release"]
            require(annotations["org.opencontainers.image.version"] == version, "release annotation mismatch")
            prefix = directory + expected["architecture"]
            manifest_name, config_name = prefix + "-manifest.json", prefix + "-config.json"
            expected_files.update((manifest_name, config_name))
            manifest = checked_json(evidence[manifest_name], descriptor["digest"], descriptor["size"])
            require(manifest["schemaVersion"] == 2 and manifest["mediaType"] == MANIFEST_TYPE, "invalid OCI manifest")
            config_descriptor = manifest["config"]
            require(config_descriptor["mediaType"] == CONFIG_TYPE, "unsupported image config")
            require(config_descriptor["digest"] == platform["configDigest"], "config not selected by manifest")
            config_raw = evidence[config_name]
            config = checked_json(config_raw, config_descriptor["digest"], config_descriptor["size"])
            if "data" in config_descriptor:
                require(base64.b64decode(config_descriptor["data"], validate=True) == config_raw, "embedded config differs")
            actual = {key: config[key] for key in ("os", "architecture", "variant") if key in config}
            require(actual == expected, "config platform mismatch")
            require(isinstance(manifest["layers"], list) and 1 <= len(manifest["layers"]) <= 128, "layer descriptor limit")
            for layer in manifest["layers"]:
                require(re.fullmatch(r"sha256:[0-9a-f]{64}", layer["digest"]) is not None, "layer descriptor digest")
                require(type(layer["size"]) is int and layer["size"] > 0, "layer descriptor size")
    require(set(evidence) == expected_files, "missing or unreviewed evidence files")


def load_fixture():
    directory = ROOT / "images"
    def read(path, limit=MAX_FILE_BYTES):
        require(path.is_file() and not path.is_symlink(), "evidence must be regular local files")
        with path.open("rb") as stream:
            raw = stream.read(limit + 1)
        require(len(raw) <= limit, "metadata file byte limit")
        return raw

    schema = decode(read(directory / "catalog.schema.json", 65536))
    catalog = decode(read(directory / "catalog.json", 65536))
    local_schema(schema).validate(catalog)
    evidence = {}
    for identity in catalog["images"]:
        image_directory = directory / "evidence" / identity
        require(image_directory.is_dir() and not image_directory.is_symlink(), "evidence directory must be local")
        for path in image_directory.iterdir():
            raw = read(path)
            evidence[f"{identity}/{path.name}"] = raw
            require(sum(map(len, evidence.values())) <= MAX_EVIDENCE_BYTES, "aggregate evidence byte limit")
    collection = decode(read(directory / "evidence/collection.json", 65536))
    require(collection["method"] == "registry-metadata-only", "unsupported collection method")
    require(collection["layersFetched"] is False and collection["imagesUnpackedBuiltInstalledOrRun"] is False,
            "metadata review must not claim image acquisition/runtime evidence")
    return catalog, schema, evidence


class BaseImageCatalog(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.original, cls.schema, cls.original_evidence = load_fixture()

    def setUp(self):
        self.catalog = copy.deepcopy(self.original)
        self.evidence = self.original_evidence.copy()

    def check(self):
        validate(self.catalog, self.schema, self.evidence)

    def rewrite_index(self, index):
        raw = json.dumps(index, separators=(",", ":")).encode()
        self.evidence["debian-13/index.json"] = raw
        self.evidence["debian-13/index-by-digest.json"] = raw
        self.catalog["images"]["debian-13"]["indexDigest"] = digest(raw)

    def rewrite_manifest(self, manifest):
        raw = json.dumps(manifest, separators=(",", ":")).encode()
        self.evidence["debian-13/amd64-manifest.json"] = raw
        self.catalog["images"]["debian-13"]["platforms"]["x86_64-linux"]["manifestDigest"] = digest(raw)
        index = decode(self.evidence["debian-13/index.json"])
        for entry in index["manifests"]:
            if entry.get("platform") == PLATFORMS["x86_64-linux"]:
                entry.update(digest=digest(raw), size=len(raw))
        self.rewrite_index(index)

    def test_reviewed_metadata_chains(self):
        self.check()
        self.assertEqual(set(self.catalog["images"]), {"debian-13", "ubuntu-26.04"})
        self.assertEqual(len(self.evidence), 12)

    def test_raw_bytes_are_hashed_before_json_normalization(self):
        self.evidence["debian-13/amd64-manifest.json"] += b"\n"
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            self.check()

    def test_index_refetch_must_match(self):
        self.evidence["debian-13/index-by-digest.json"] += b"\n"
        with self.assertRaisesRegex(ValueError, "roundtrip mismatch"):
            self.check()

    def test_descriptor_size_is_checked_even_with_a_valid_digest(self):
        index = decode(self.evidence["debian-13/index.json"])
        next(entry for entry in index["manifests"] if entry.get("platform") == PLATFORMS["x86_64-linux"])["size"] += 1
        self.rewrite_index(index)
        with self.assertRaisesRegex(ValueError, "size mismatch"):
            self.check()

    def test_duplicate_selected_platform_is_ambiguous(self):
        index = decode(self.evidence["debian-13/index.json"])
        index["manifests"].append(next(entry for entry in index["manifests"] if entry.get("platform") == PLATFORMS["x86_64-linux"]))
        self.rewrite_index(index)
        with self.assertRaisesRegex(ValueError, "ambiguous platform"):
            self.check()

    def test_platform_config_is_checked_after_the_full_digest_chain(self):
        config = decode(self.evidence["debian-13/amd64-config.json"])
        config["architecture"] = "arm64"
        raw = json.dumps(config, separators=(",", ":")).encode()
        self.evidence["debian-13/amd64-config.json"] = raw
        self.catalog["images"]["debian-13"]["platforms"]["x86_64-linux"]["configDigest"] = digest(raw)
        manifest = decode(self.evidence["debian-13/amd64-manifest.json"])
        manifest["config"].update(digest=digest(raw), size=len(raw), data=base64.b64encode(raw).decode())
        self.rewrite_manifest(manifest)
        with self.assertRaisesRegex(ValueError, "config platform mismatch"):
            self.check()

    def test_embedded_config_is_not_a_second_source(self):
        manifest = decode(self.evidence["debian-13/amd64-manifest.json"])
        manifest["config"]["data"] = base64.b64encode(b"{}").decode()
        self.rewrite_manifest(manifest)
        with self.assertRaisesRegex(ValueError, "embedded config differs"):
            self.check()

    def test_catalog_manifest_must_be_selected_by_index(self):
        self.catalog["images"]["debian-13"]["platforms"]["x86_64-linux"]["manifestDigest"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(ValueError, "not selected"):
            self.check()

    def test_layers_or_other_unreviewed_files_are_not_evidence(self):
        self.evidence["debian-13/layer.tar"] = b"not metadata"
        with self.assertRaisesRegex(ValueError, "unreviewed evidence"):
            self.check()

    def test_catalog_refuses_extra_authority_and_mutable_digests(self):
        from jsonschema.exceptions import ValidationError

        for field, value in (("indexDigest", "trixie-20261005"), ("ready", True)):
            with self.subTest(field=field):
                catalog = copy.deepcopy(self.original)
                catalog["images"]["debian-13"][field] = value
                with self.assertRaises(ValidationError):
                    validate(catalog, self.schema, self.original_evidence)

    def test_schema_and_json_cannot_hide_alternate_authority(self):
        schema = copy.deepcopy(self.schema)
        schema["$defs"]["unused"] = {"$ref": "https://example.invalid/schema.json"}
        with self.assertRaisesRegex(ValueError, "nonlocal"):
            local_schema(schema)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            decode(b'{"digest":"one","digest":"two"}')


if __name__ == "__main__":
    unittest.main()
