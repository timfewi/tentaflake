#!/usr/bin/env python3
"""Validate every flake output in sequential, small Nix evaluators."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


INVENTORY = r'''
let
  source = (builtins.getFlake (builtins.getEnv "TENTAFLAKE_EVAL_PROJECTION")).inputs.source;
in {
  outputs = builtins.attrNames source.outputs;
  checks = builtins.mapAttrs (_: builtins.attrNames) (source.checks or {});
  packages = builtins.mapAttrs (_: builtins.attrNames) (source.packages or {});
  hosts = builtins.attrNames (source.nixosConfigurations or {});
}
'''

PROJECTION = r'''
{
  inputs.source.url = @SOURCE@;
  outputs = { source, ... }:
    let
      selection = builtins.fromJSON (builtins.readFile ./selection.json);
      outputs = source.outputs;
      select = output: groups: builtins.mapAttrs (system: names:
        builtins.listToAttrs (map (name: {
          inherit name;
          value = outputs.${output}.${system}.${name};
        }) names)
      ) groups;
    in
    if selection.kind == "host" then {
      nixosConfigurations.${selection.name} = outputs.nixosConfigurations.${selection.name};
      checks = select "checks" selection.checks;
      packages = select "packages" selection.packages;
    } else if selection.kind == "checks" then {
      checks = select "checks" selection.checks;
    } else if selection.kind == "packages" then {
      packages = select "packages" selection.packages;
    } else if selection.kind == "empty" then {} else {
      ${selection.output} = outputs.${selection.output};
    };
}
'''


def evaluation_plan(inventory):
    """Pair host aliases before partitioning the remaining complete inventory."""
    checks = {system: list(names) for system, names in inventory["checks"].items()}
    packages = {system: list(names) for system, names in inventory["packages"].items()}
    plan = []
    for host in inventory["hosts"]:
        selection = {"kind": "host", "name": host, "checks": {}, "packages": {}}
        # Tentaflake's host check and installer ISO package both alias hosts.
        for output, groups in (("checks", checks), ("packages", packages)):
            for system, names in groups.items():
                if host in names:
                    selection[output][system] = [host]
                    names.remove(host)
        plan.append(selection)
    for system, names in checks.items():
        if not names:
            plan.append({"kind": "checks", "checks": {system: []}})
        for offset in range(0, len(names), 4):
            plan.append({"kind": "checks", "checks": {system: names[offset:offset + 4]}})
    for output in inventory["outputs"]:
        if output == "checks" and (checks or inventory["hosts"]):
            continue
        if output == "nixosConfigurations" and inventory["hosts"]:
            continue
        if output == "packages":
            plan.append({"kind": "packages", "packages": packages})
        else:
            plan.append({"kind": "output", "output": output})
    return plan or [{"kind": "empty"}]


def evaluate(repository):
    # A Git input retains ordinary tracked-source filtering, including dirty
    # tracked edits. Never snapshot the checkout with path: (ignored state leaks).
    source = "git+" + repository.as_uri()
    with tempfile.TemporaryDirectory(prefix="tentaflake-eval-") as directory:
        work = Path(directory)
        (work / "flake.nix").write_text(PROJECTION.replace("@SOURCE@", json.dumps(source)))
        (work / "selection.json").write_text('{"kind":"empty"}')
        projection = "path:" + str(work)
        # Only this disposable wrapper gets a lock; the source's pins are reused.
        subprocess.run(
            ["nix", "flake", "lock", "--allow-dirty-locks", projection],
            cwd=repository, check=True,
        )
        result = subprocess.run(
            ["nix", "eval", "--json", "--no-write-lock-file", "--impure", "--expr", INVENTORY],
            cwd=repository, check=True, capture_output=True, text=True,
            env=os.environ | {"TENTAFLAKE_EVAL_PROJECTION": str(work)},
        )
        if result.stderr:
            print(result.stderr, file=sys.stderr, end="")
        plan = evaluation_plan(json.loads(result.stdout))
        for index, selection in enumerate(plan, 1):
            (work / "selection.json").write_text(json.dumps(selection))
            print(f"Evaluating flake projection {index}/{len(plan)}: {json.dumps(selection)}", flush=True)
            # Keep upstream output/type/derivation validation; merely evaluating
            # drvPath would lose that coverage. Each process exits before the next.
            subprocess.run(
                ["nix", "flake", "check", "--no-build", "--no-write-lock-file", projection],
                cwd=repository, check=True,
            )


def main():
    if len(sys.argv) != 1:
        print("Usage: python3 scripts/eval_flake.py", file=sys.stderr)
        return 2
    try:
        evaluate(Path(__file__).resolve().parents[1])
    except subprocess.CalledProcessError as error:
        if error.stderr:
            print(error.stderr, file=sys.stderr, end="")
        return error.returncode if error.returncode > 0 else 1
    except (OSError, ValueError) as error:
        print(f"Cannot evaluate flake projections: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
