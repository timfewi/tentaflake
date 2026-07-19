#!/usr/bin/env bash
# Re-resolve the agent image digests pinned in lib/constants.nix.
#
# Prints the current multi-arch index digest for each upstream tag. Pinning is
# only useful if bumping it is easy — but the bump stays MANUAL on purpose: a
# script that rewrites the pin automatically is just a mutable tag with extra
# steps. Review the new digest, then edit lib/constants.nix by hand.
set -euo pipefail

# tag references to track; keep in sync with lib/constants.nix
IMAGES=(
  "docker://docker.io/nousresearch/hermes-agent:latest"
  "docker://ghcr.io/zeroclaw-labs/zeroclaw:v0.8.2"
  "docker://ghcr.io/anomalyco/opencode:latest"
)

if command -v skopeo >/dev/null 2>&1; then
  SKOPEO=(skopeo)
else
  SKOPEO=(nix run nixpkgs#skopeo --)
fi

for ref in "${IMAGES[@]}"; do
  # capture first, then match the "Digest" field — piping into `head` trips
  # SIGPIPE under `set -o pipefail`, and a bare digest grep could match a layer
  out=$("${SKOPEO[@]}" inspect --no-tags "$ref")
  digest=$(grep -m1 '"Digest"' <<<"$out" | grep -o 'sha256:[0-9a-f]\{64\}')
  # strip scheme, then the tag — podman/skopeo reject `repo:tag@digest`, so the
  # pinned reference must carry the digest alone
  repo="${ref#docker://}"
  repo="${repo%:*}"
  printf '%s\n  %s@%s\n\n' "$ref" "$repo" "$digest"
done

echo "Compare against lib/constants.nix and update by hand if these differ."
