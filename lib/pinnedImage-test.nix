# Runnable check for lib/pinnedImage.nix — `nix flake check` fails if the
# digest guard stops rejecting mutable tags.
{ pkgs }:
let
  inherit (pkgs) lib;
  constants = import ./constants.nix;
  pinnedImage = import ./pinnedImage.nix { inherit lib; };
  baseImages = import ./baseImages.nix { inherit lib; };

  # bare hex — call sites add the "sha256:" prefix themselves
  digest = "4a2f23bd3ffaa6ee7b3be8a302a38be43ab0321a2988cd3fb16b7dd472dde812";

  # returns true if pinnedImage threw
  rejects = image: !(builtins.tryEval (pinnedImage "test" false image)).success;
  accepts =
    image:
    (builtins.tryEval (pinnedImage "test" false image)) == {
      success = true;
      value = image;
    };

  cases = [
    # mutable tags are rejected
    (rejects "nousresearch/hermes-agent:latest")
    (rejects "ghcr.io/zeroclaw-labs/zeroclaw:v0.8.2")
    (rejects "ubuntu")
    # near-misses must not sneak through
    (rejects "repo@sha256:deadbeef") # too short
    (rejects "repo@sha512:${lib.concatStrings (lib.genList (_: "a") 64)}") # wrong algo
    (rejects "repo@sha256:${lib.concatStrings (lib.genList (_: "g") 64)}") # non-hex
    (rejects "repo@sha256:${digest}-extra") # trailing junk
    # tag+digest parses for docker but NOT for podman/skopeo — reject it
    (rejects "ghcr.io/zeroclaw-labs/zeroclaw:v0.8.2@sha256:${digest}")
    # shell-injection: nixpkgs splices `image` into the unit's shell command
    # unquoted, so anything that word-splits must never look "pinned"
    (rejects "evil/backdoor x@sha256:${digest}")
    (rejects "evil/backdoor\nfoo@sha256:${digest}")
    (rejects "evil/backdoor\tx@sha256:${digest}")
    (rejects "--privileged -v /:/host_root evil/backdoor x@sha256:${digest}")
    (rejects "$(id > /tmp/pwned) evil/backdoor x@sha256:${digest}")
    (rejects "repo; poweroff@sha256:${digest}")
    # ...and the opt-out must NOT be an escape from shell safety
    (!(builtins.tryEval (pinnedImage "test" true "$(id) evil/img")).success)
    (!(builtins.tryEval (pinnedImage "test" true "--privileged evil/img")).success)
    # plain digests are accepted
    (accepts "nousresearch/hermes-agent@sha256:${digest}")
    (accepts "ghcr.io/zeroclaw-labs/zeroclaw@sha256:${digest}")
    # a registry port is a colon that is NOT a tag
    (accepts "localhost:5000/my-agent@sha256:${digest}")
    # the opt-out works for a legitimate locally-built tag
    (builtins.tryEval (pinnedImage "test" true "my-agent:local")).success
    # the shipped defaults are pinned
    (accepts constants.hermesImage)
    (accepts constants.zeroclawImage)
    # Base selection returns a reviewed platform manifest, never the index/tag.
    (lib.all (
      id:
      lib.all (
        system:
        let
          entry = baseImages.catalog.images.${id};
          selected = baseImages.image { inherit id system; };
        in
        accepts selected
        && selected == "${entry.repository}@${entry.platforms.${system}.manifestDigest}"
        && selected != "${entry.repository}@${entry.indexDigest}"
      ) (builtins.attrNames baseImages.catalog.images.${id}.platforms)
    ) (builtins.attrNames baseImages.catalog.images))
    (
      !(builtins.tryEval (
        baseImages.image {
          id = "missing";
          system = "x86_64-linux";
        }
      )).success
    )
    (
      !(builtins.tryEval (
        baseImages.image {
          id = "debian-13";
          system = "powerpc64le-linux";
        }
      )).success
    )
  ];

  failed = lib.length (lib.filter (ok: !ok) cases);
in
assert failed == 0 || throw "pinnedImage: ${toString failed} case(s) failed";
pkgs.runCommand "image-pinning-test" { } "touch $out"
