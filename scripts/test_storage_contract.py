#!/usr/bin/env python3
"""Offline Rust/Nix storage parity and real backup-module projection regression.

Run with an already-built CLI and the already-present pinned nixpkgs source:
  python3 scripts/test_storage_contract.py --cli /path/to/tentaflake --nixpkgs /path/to/nixpkgs

Evaluates the pinned NixOS option graph with the real backup, Restic, systemd,
quota and agent-metadata modules. Quota dependencies use a stopped synthetic OCI
declaration, not a vendor image or imported agent runtime. Only the real security
option declarations are loaded, not full host security policy; this is bounded
storage-projection evidence, not host-security, VM, backup or restore acceptance.
Only systemFromData's storage config is evaluated, not its agent imports.
No build, activation, repository initialization or backup job is performed.
"""

import argparse
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PATH = "/var/lib/tentaflake/agents/coding"
BASE_STORAGE = {
    "enable": True,
    "paths": [DEFAULT_PATH],
    "repositoryFile": "/run/agenix/restic-repository",
    "passwordFile": "/run/agenix/restic-password",
}
CUSTOM_AGENT = {
    "name": "kept",
    "adapter": "hermes",
    "autoStart": False,
    "stateDir": "/var/lib/private/kept",
}


def desired(storage, agents=None):
    value = {"schemaVersion": 1, "agents": agents or []}
    if storage is not None:
        value["storage"] = copy.deepcopy(storage)
    return value


def encode(value):
    lines = ["schemaVersion = 1"]
    if not value["agents"]:
        lines.append("agents = []")
    for agent in value["agents"]:
        lines.append("[[agents]]")
        lines.extend(f"{key} = {json.dumps(item)}" for key, item in agent.items())
    if "storage" in value:
        lines.append("[storage]")
        lines.extend(f"{key} = {json.dumps(item)}" for key, item in value["storage"].items())
    return "\n".join(lines) + "\n"


def corpus():
    cases = []

    def add(label, storage, accepted, agents=None):
        cases.append({"label": label, "value": desired(storage, agents), "accepted": accepted})

    for label, storage in [("absent", None), ("empty", {}), ("disabled", {"enable": False})]:
        add(label, storage, True)
    add("enabled", BASE_STORAGE, True)
    add("custom-state-without-storage", None, True, [CUSTOM_AGENT])
    for field in BASE_STORAGE:
        changed = copy.deepcopy(BASE_STORAGE)
        del changed[field]
        add(f"missing-{field}", changed, False)
    for field, item in BASE_STORAGE.items():
        if field != "enable":
            add(f"disabled-{field}", {"enable": False, field: item}, False)
    for field in ["environmentFile", "repository", "password", "initialize", "reset", "timerConfig", "pruneOpts", "extraBackupArgs"]:
        changed = copy.deepcopy(BASE_STORAGE)
        changed[field] = "/run/agenix/backend" if field == "environmentFile" else "DO-NOT-PRINT-STORAGE"
        add(f"field-{field}", changed, field == "environmentFile")
    add("disabled-environment", {"enable": False, "environmentFile": "/run/agenix/backend"}, False)
    for paths, accepted in [
        ([], False),
        ([DEFAULT_PATH, DEFAULT_PATH], False),
        ([DEFAULT_PATH + "/workspace"], True),
        (["/var/lib/tentaflake-broker-llm-agent-coding"], True),
        (["/var/lib/tentaflake/agents/" + "a" * 57], True),
        (["/var/lib/tentaflake/agents/" + "a" * 58], False),
        ([f"/var/lib/tentaflake/agents/a{i}" for i in range(128)], True),
        ([f"/var/lib/tentaflake/agents/a{i}" for i in range(129)], False),
    ]:
        changed = copy.deepcopy(BASE_STORAGE)
        changed["paths"] = paths
        add(f"path-list-{len(cases)}", changed, accepted)
    for path in [
        "/", "/var/lib", "/var/lib/", "/var/lib/tentaflake/agents",
        "/var/lib/private/kept", "/var/lib/docker", "/var/lib/containers/state",
        DEFAULT_PATH + "/", DEFAULT_PATH + "//workspace", DEFAULT_PATH + "/../other",
        DEFAULT_PATH + "/./workspace", DEFAULT_PATH + "/arbitrary", "relative",
        "${builtins.readFile /run/secret}", "DO-NOT-PRINT-STORAGE",
    ]:
        changed = copy.deepcopy(BASE_STORAGE)
        changed["paths"] = [path]
        add(f"unsafe-source-{len(cases)}", changed, False)
    for path, accepted in [
        (CUSTOM_AGENT["stateDir"], True),
        (CUSTOM_AGENT["stateDir"] + "/workspace", False),
        ("/var/lib/private/another", False),
    ]:
        changed = copy.deepcopy(BASE_STORAGE)
        changed["paths"] = [path]
        add(f"custom-source-{len(cases)}", changed, accepted, [CUSTOM_AGENT])
    for length, accepted in [(256, True), (257, False)]:
        path = "/var/lib/" + "a" * (length - len("/var/lib/"))
        agent = dict(CUSTOM_AGENT, stateDir=path)
        changed = dict(BASE_STORAGE, paths=[path])
        add(f"custom-length-{length}", changed, accepted, [agent])
    for field in ["repositoryFile", "passwordFile", "environmentFile"]:
        for reference, accepted in [
            ("/run/agenix/backup", True), ("/run/" + "a" * 251, True),
            ("/run/" + "a" * 252, False), ("/run", False), ("/run/", False),
            ("/run/../secret", False), ("/run/./secret", False),
            ("/run//secret", False), ("/run/secret/", False),
            ("/nix/store/secret", False), ("/etc/secret", False),
            ("s3:bucket/repository", False), ("DO-NOT-PRINT-STORAGE", False),
            ("${builtins.readFile /run/secret}", False), (False, False),
        ]:
            changed = copy.deepcopy(BASE_STORAGE)
            changed[field] = reference
            add(f"runtime-{field}-{len(cases)}", changed, accepted)
    return cases


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def run(argv):
    return subprocess.run(argv, check=False, capture_output=True, text=True, timeout=120)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--nixpkgs", type=Path, required=True)
    args = parser.parse_args()
    cli = str(args.cli.resolve(strict=True))
    nixpkgs = str(args.nixpkgs.resolve(strict=True))
    cases = corpus()
    check(len(cases) == 94, "expected the complete protected 94-case corpus")
    with tempfile.TemporaryDirectory(prefix="tentaflake-storage-contract-") as directory:
        directory = Path(directory)
        source = directory / "tentaflake.toml"
        for case in cases:
            text = encode(case["value"])
            check(tomllib.loads(text) == case["value"], "test encoder changed input")
            source.write_text(text)
            result = run([cli, "config", "validate", str(source)])
            check(result.returncode == (0 if case["accepted"] else 2), f"Rust: {case['label']}")
            if not case["accepted"]:
                check("DO-NOT-PRINT-STORAGE" not in result.stdout + result.stderr, "private input leaked")
            else:
                normalized = run([cli, "config", "normalize", str(source)])
                check(normalized.returncode == 0, f"normalize: {case['label']}")
                check(tomllib.loads(normalized.stdout) == case["value"], f"roundtrip: {case['label']}")
        fixture = dict(BASE_STORAGE, paths=[CUSTOM_AGENT["stateDir"], DEFAULT_PATH], environmentFile="/run/agenix/backend")
        source.write_text(encode(desired(fixture, [CUSTOM_AGENT])))
        corpus_file = directory / "cases.json"
        corpus_file.write_text(json.dumps(cases))
        disabled_sources = []
        for index, storage in enumerate([None, {}, {"enable": False}]):
            disabled_directory = directory / f"disabled-{index}"
            disabled_directory.mkdir()
            disabled_source = disabled_directory / "tentaflake.toml"
            disabled_source.write_text(encode(desired(storage)))
            disabled_sources.append(str(disabled_source))
        expression = """
let
  root = builtins.toPath ROOT;
  nixpkgs = builtins.toPath NIXPKGS;
  system = builtins.currentSystem;
  pkgs = import nixpkgs { inherit system; };
  inherit (pkgs) lib;
  validate = import (root + "/lib/systemSchema.nix") { inherit lib; };
  cases = builtins.fromJSON (builtins.readFile (builtins.toPath CORPUS));
  accepts = value: (builtins.tryEval (builtins.deepSeq (validate value) true)).success;
  compile = file: (import (root + "/lib/systemFromData.nix") { inherit pkgs lib; }) { inherit file; };
  evalConfig = import (nixpkgs + "/nixos/lib/eval-config.nix");
  evaluate = file: extra: (evalConfig {
    inherit system pkgs;
    modules = [
      (root + "/modules/backup.nix")
      (root + "/modules/workspace-quota.nix")
      (root + "/modules/agent-instances.nix")
      ({ config, ... }: {
        # Reuse the declared profile option without claiming full security policy
        # coverage: this fixture evaluates the storage projection only.
        options.tentaflake.security =
          (import (root + "/modules/security.nix") {
            inherit config lib pkgs;
          }).options.tentaflake.security;
      })
      {
        system.stateVersion = "26.05";
        fileSystems."/" = {
          device = "/dev/disk/by-label/nixos";
          fsType = "btrfs";
        };
        boot.loader.grub.devices = [ "nodev" ];
        # Real declared options, synthetic stopped dependencies. No image is
        # pulled, quota filesystem mounted, or runtime service executed.
        virtualisation.oci-containers.containers.selected = {
          image = "localhost/tentaflake-storage-fixture:synthetic";
          autoStart = false;
        };
        tentaflake.agentDeclarations = [{
          adapter = "generic";
          name = "selected";
          container = "selected";
          unit = "tentaflake-storage-fixture.service";
          stateDir = "/var/lib/private/kept";
          stateStorage = "/var/lib/private/kept/state";
          workspace = "/var/lib/private/kept/workspace";
          uid = 1000;
          gid = 1000;
          runnable = false;
        }];
        tentaflake.workspaceQuota.agents.selected = {
          enable = true;
          workspace = "/var/lib/private/kept/workspace";
          state.path = "/var/lib/private/kept/state";
        };
      }
      # Preserve the original storage-only projection, excluding agent imports.
      { inherit (compile file) config; }
    ] ++ extra;
  }).config;
  config = evaluate (builtins.toPath FIXTURE) [ ];
  backup = config.services.restic.backups.tentaflake;
in {
  accepted = map (case: accepts case.value) cases;
  # Serialize only the public fields consumed by the original assertions, not
  # package derivations or internal values added by the real Restic module.
  backup = {
    inherit (backup)
      repositoryFile passwordFile environmentFile paths initialize runCheck extraBackupArgs;
  };
  assertions = map (assertion: assertion.assertion) config.assertions;
  mounts = config.systemd.services.restic-backups-tentaflake.unitConfig;
  disabled = map (file:
    let
      inactive = evaluate (builtins.toPath file) [ ];
      consumer = evaluate (builtins.toPath file) [{
        tentaflake.backup = {
          enable = true;
          paths = [ "/var/lib/consumer/state" ];
          repositoryFile = "/run/agenix/consumer-repository";
          passwordFile = "/run/agenix/consumer-password";
        };
      }];
    in {
      staysDisabled = !inactive.tentaflake.backup.enable;
      noJob = lib.attrByPath [ "services" "restic" "backups" "tentaflake" ] null inactive == null;
      consumerPaths = consumer.services.restic.backups.tentaflake.paths;
    }
  ) (builtins.fromJSON DISABLED);
}
"""
        for key, value in [
            ("ROOT", str(ROOT)),
            ("NIXPKGS", nixpkgs),
            ("CORPUS", str(corpus_file)),
            ("FIXTURE", str(source)),
            ("DISABLED", json.dumps(disabled_sources)),
        ]:
            expression = expression.replace(key, json.dumps(value))
        evaluated = run(["nix", "eval", "--offline", "--impure", "--json", "--expr", expression])
        check(evaluated.returncode == 0, f"Nix evaluation failed: {evaluated.stderr}")
        result = json.loads(evaluated.stdout)
        for case, accepted in zip(cases, result["accepted"], strict=True):
            check(accepted == case["accepted"], f"Nix: {case['label']}")
        backup = result["backup"]
        for field in ["repositoryFile", "passwordFile", "environmentFile"]:
            check(backup[field] == fixture[field], f"projection: {field}")
        expected_paths = fixture["paths"] + ["/var/lib/private/kept/workspace", "/var/lib/private/kept/state"]
        check(backup["paths"] == expected_paths, "explicit sources/selected quota mounts changed")
        check(backup["initialize"] is False, "public backup initialized a repository")
        check(backup["runCheck"] is True, "integrity check disabled")
        check(backup["extraBackupArgs"] == ["--one-file-system"], "filesystem boundary changed")
        check(all(result["assertions"]), "real backup-module assertion failed")
        check(result["mounts"]["RequiresMountsFor"] == expected_paths, "required mounts changed")
        check(result["mounts"]["AssertPathIsMountPoint"] == expected_paths[2:], "quota mount assertions changed")
        for disabled in result["disabled"]:
            check(disabled["staysDisabled"] and disabled["noJob"], "disabled storage created a backup job")
            check(disabled["consumerPaths"] == ["/var/lib/consumer/state"], "disabled storage overrode consumer backup policy")
    print(f"storage contract: {len(cases)} Rust/Nix cases, normalization and real backup projection passed")


if __name__ == "__main__":
    main()
