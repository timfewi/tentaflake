# Reviewed base OS metadata, independent of native agent presets and acquisition.
{ lib }:
let
  catalog = builtins.fromJSON (builtins.readFile ../images/catalog.json);
  pinnedImage = import ./pinnedImage.nix { inherit lib; };
in
assert catalog.schemaVersion == 1 && builtins.isAttrs catalog.images;
{
  inherit catalog;

  # Select the exact reviewed platform manifest, never a mutable discovery tag
  # or an unreviewed platform from the multi-platform index. No fetch occurs.
  image =
    { id, system }:
    let
      entry = catalog.images.${id} or (throw "tentaflake: unknown base image '${id}'.");
      platform =
        entry.platforms.${system}
          or (throw "tentaflake: base image '${id}' has no reviewed '${system}' manifest.");
    in
    pinnedImage "base-${id}" false "${entry.repository}@${platform.manifestDigest}";
}
