#!/usr/bin/env python3
"""Additional synthetic Rust/Nix storage evidence using declared NixOS options.

Run with an already-built CLI and the already-present pinned nixpkgs source:
  python3 scripts/test_storage_contract_nixos.py --cli /path/to/tentaflake --nixpkgs /path/to/nixpkgs

Reuses the protected test_storage_contract.py corpus and helpers, preserving
all of its validation, normalization, redaction and projection assertions.
The original runner remains unchanged and its reported root-freeform recursion
failure is not repaired or superseded by a passing result from this runner.

The backup, Restic and systemd modules are real; quota metadata is a synthetic,
explicitly typed fixture, not an evaluated quota/runtime implementation. Only
systemFromData's storage config is evaluated: agent imports are deliberately
excluded, as in the original fixture. This is finite-corpus and backup-module
projection evidence, not a full-system, VM, restore or vendor acceptance test.
No build, activation, repository initialization or backup job is performed.
"""

import argparse
import json
from pathlib import Path
import tempfile
import tomllib

from test_storage_contract import (
    BASE_STORAGE,
    CUSTOM_AGENT,
    DEFAULT_PATH,
    ROOT,
    check,
    corpus,
    desired,
    encode,
    run,
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--nixpkgs", type=Path, required=True)
    args = parser.parse_args()
    cli = str(args.cli.resolve(strict=True))
    nixpkgs = str(args.nixpkgs.resolve(strict=True))
    cases = corpus()
    check(len(cases) == 94, "expected the complete protected 94-case corpus")
    with tempfile.TemporaryDirectory(prefix="tentaflake-storage-nixos-contract-") as directory:
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
      {
        system.stateVersion = "26.05";
        fileSystems."/" = {
          device = "/dev/disk/by-label/nixos";
          fsType = "btrfs";
        };
        boot.loader.grub.devices = [ "nodev" ];
      }
      {
        # Synthetic metadata only: no quota implementation or runtime is imported.
        options.tentaflake.workspaceQuota.agents = lib.mkOption {
          type = lib.types.attrsOf (lib.types.submodule {
            options = {
              enable = lib.mkEnableOption "synthetic quota mount metadata";
              workspace = lib.mkOption { type = lib.types.str; };
              state = lib.mkOption {
                type = lib.types.nullOr (lib.types.submodule {
                  options.path = lib.mkOption { type = lib.types.str; };
                });
                default = null;
              };
            };
          });
          default.selected = {
            enable = true;
            workspace = "/var/lib/private/kept/workspace";
            state.path = "/var/lib/private/kept/state";
          };
        };
      }
      # Scope matches the original: project storage config, not agent imports.
      { inherit (compile file) config; }
    ] ++ extra;
  }).config;
  config = evaluate (builtins.toPath FIXTURE) [ ];
  backup = config.services.restic.backups.tentaflake;
in {
  accepted = map (case: accepts case.value) cases;
  # Select public assertion inputs; do not serialize packages or module internals.
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
    print(
        f"additional synthetic storage evidence: {len(cases)} Rust/Nix cases, normalization "
        "and declared-NixOS backup projection passed (synthetic quota metadata; "
        "original test_storage_contract.py unchanged and not repaired)"
    )


if __name__ == "__main__":
    main()
