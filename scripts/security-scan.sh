#!/usr/bin/env bash
# Scan tracked source with pinned Semgrep rules and both dependency lockfiles
# against OSV's current advisory database.
set -euo pipefail

repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo_dir"

flake_ref=${TENTAFLAKE_FLAKE_REF:-.}
tools_root=$(nix build \
  --no-link \
  --print-out-paths \
  "$flake_ref#security-tools")
rules_root=$(nix build \
  --no-link \
  --print-out-paths \
  "$flake_ref#semgrep-rules")
devcontainer_source=$(nix eval \
  --raw \
  "$flake_ref#devcontainer-cli.src")

echo "==> Scanning Rust dependencies with OSV"
"$tools_root/bin/osv-scanner" scan source --lockfile Cargo.lock

echo "==> Scanning Dev Containers CLI dependencies with OSV"
"$tools_root/bin/osv-scanner" scan source \
  --lockfile "$devcontainer_source/yarn.lock"

rule_args=()
rule_directories=(
  rust/lang/security
  bash/curl/security
  bash/lang/security
  dockerfile/security
  yaml/github-actions/security
  generic/secrets/gitleaks
)

for directory in "${rule_directories[@]}"; do
  while IFS= read -r rule; do
    rule_args+=(--config "$rule")
  done < <(
    # The mutable-action-tag rule already requires exact action SHAs. Its older
    # audit duplicate emits a Semgrepignore-v2 compatibility warning.
    find "$rules_root/$directory" \
      -type f \
      \( -name '*.yml' -o -name '*.yaml' \) \
      ! -name '*.test.*' \
      ! -name '*.fixed.*' \
      ! -name 'third-party-action-not-pinned-to-commit-sha.yml' \
      -print | sort
  )
done

echo "==> Scanning tracked source with Semgrep"
SEMGREP_SEND_METRICS=off "$tools_root/bin/semgrep" scan \
  --disable-version-check \
  --error \
  --strict \
  --metrics=off \
  --severity WARNING \
  --severity ERROR \
  "${rule_args[@]}" \
  .

echo "==> Security scan passed"
