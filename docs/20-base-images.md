# Pinned base images

[`images/catalog.json`](../images/catalog.json) owns the reviewed Debian 13 and
Ubuntu 26.04 LTS base-image metadata. These are operating-system bases;
[`adapters/catalog.json`](../adapters/catalog.json) continues to own native agent
presets, commands and acceptance facts. Existing agent pins are unchanged.

Each entry records its dated discovery tag, multi-platform index digest and
reviewed manifest/config digests for `linux/amd64` and `linux/arm64/v8`. Tags
are discovery evidence. The Nix helper returns a repository reference pinned
to the selected platform manifest:

```nix
let
  baseImages = tentaflake.lib.x86_64-linux.baseImages;
in
baseImages.image {
  id = "debian-13";
  system = "x86_64-linux";
}
```

Use `id = "ubuntu-26.04"` for Ubuntu or `system = "aarch64-linux"` for its
reviewed ARM64 manifest. Unknown IDs and unreviewed platforms fail evaluation.
Selection performs no fetch. The installed template also copies `images/` and
exports this helper through `lib/default.nix`. ARM64 metadata review does not
establish ARM64 host support.

## Review evidence

The 2026-10-13 collection used Skopeo 1.23.0 from pinned nixpkgs, TLS verification,
no registry credentials and bounded metadata requests. The sanitized
[`collection.json`](../images/evidence/collection.json) records the client source,
command forms and collection limits. Twelve raw files total 45,878 bytes:
each discovery index, its immutable digest refetch, and the two selected
platform manifests and configs. Publisher source URLs come from OCI annotations.

The offline validator checks raw-byte SHA-256 before decoding JSON, exact
descriptor sizes, the index roundtrip, unique platform selection and config
platforms. Embedded config data must equal the saved config bytes. Other
platforms and attestation descriptors present in an index are outside this
review. Preserve raw evidence bytes; reformatting changes their digests.

No image layers were acquired, unpacked, built, installed or run. This evidence
does not establish image signatures, package patch levels, native tools,
model/Research integration or runtime containment. The existing
[Cosign image gate](10-security-profiles.md#image-signatures-and-provenance)
remains the signature-policy owner. A digest chain and publisher annotations
do not verify a signing identity.

## Checks and updates

Run `just image-catalog` for local schema, metadata-chain and negative regression
checks. It is included in `just fast` and static CI. The existing Nix
`image-pinning` check covers selection and refusal of unknown IDs/platforms.
None of these checks downloads layers or starts containers.

An update requires an explicit reviewed discovery tag, raw index plus its digest
refetch, and the selected manifest/config bytes. Update the catalog and collection
record together, then review and validate their digest chains. Layer acquisition,
signature verification and runtime acceptance remain separate operations.
Artifact acquisition belongs to the host image workflow; it does not use or
expand the agent Research transport or grant capsule Internet access.

The [host API](19-host-api.md) wire contract is unchanged. A future backend must
project the host's selected platform into its closed `Image` schema and provide
current availability and evidence. The internal catalog is not an API response;
metadata review alone cannot mark an image available. This change adds no
backend or image-acquisition service.
