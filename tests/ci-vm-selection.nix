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
        ../.github/ci-paths.json
        ../.github/workflows/check.yml
        ../scripts/ci_changes.py
        ../scripts/test_ci_changes.py
        ../scripts/eval_flake.py
        ../scripts/test_eval_flake.py
      ];
    };
  }
  ''
    python3 "$src/scripts/test_ci_changes.py"
    python3 "$src/scripts/test_eval_flake.py"
    touch "$out"
  ''
