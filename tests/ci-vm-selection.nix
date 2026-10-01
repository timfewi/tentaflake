{ pkgs }:
pkgs.runCommand "tentaflake-ci-vm-selection"
  {
    nativeBuildInputs = [
      pkgs.python3
      pkgs.gitMinimal
    ];
    src = pkgs.lib.fileset.toSource {
      root = ../.;
      fileset = pkgs.lib.fileset.unions [
        ../.github/vm-paths.json
        ../scripts/ci_vm_changes.py
        ../scripts/test_ci_vm_changes.py
      ];
    };
  }
  ''
    python3 "$src/scripts/test_ci_vm_changes.py"
    touch "$out"
  ''
