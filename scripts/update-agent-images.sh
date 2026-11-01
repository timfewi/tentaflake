#!/usr/bin/env bash
# Report candidate image pins; adapter sources own metadata, never update it here.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CATALOG="$ROOT/adapters/catalog.json"
# Validate all pins and source tags before contacting any registry. The generated
# catalog projects adapter-owned facts; scaffold and caller-provided artifacts skip.
records=$(jq -er '
  [.presets | to_entries[] | select(.value.artifact.kind == "oci") |
    .key as $id | .value.artifact |
    if (.reference | test("^[^[:space:]@]+@sha256:[0-9a-f]{64}$"))
       and (.source | type == "string")
       and (.source | test("^[^[:space:]@]+:[^/:[:space:]@]+$"))
       and ((.source | sub(":[^/:]+$"; "")) == (.reference | split("@")[0]))
    then [$id, .source, .reference] | @tsv
    else error("invalid adapter image pin/source: " + $id) end
  ] | if length > 0 then .[] else error("no OCI adapter images") end
' "$CATALOG")

if ! command -v skopeo >/dev/null 2>&1; then
  echo "skopeo is required; provide an installed or repository-pinned executable on PATH." >&2
  exit 1
fi

while IFS=$'\t' read -r adapter source current; do
  out=$(skopeo inspect --no-tags "docker://$source")
  digest=$(jq -er '.Digest | select(test("^sha256:[0-9a-f]{64}$"))' <<<"$out")
  repo="${current%@*}"
  printf '%s (%s)\n  current:   %s\n  candidate: %s@%s\n\n' "$adapter" "$source" "$current" "$repo" "$digest"
done <<<"$records"

echo "Review candidates, update the owning adapters/<id>.nix, then regenerate adapters/catalog.json."
