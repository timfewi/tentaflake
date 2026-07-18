# ────────────────────────────────────────────────────────────
# pinnedImage.nix — reject mutable OCI image references
#
# A tag (`:latest`, `:v0.8.2`) is a moving pointer: whoever controls the
# registry can repoint it at different bytes at any time, and Docker will
# happily pull those bytes on the next rebuild. Two evaluations of the same
# flake can then run different agent code — the supply-chain hole this guard
# closes. Only `@sha256:<digest>` names immutable content.
#
# Refresh pinned digests with ./scripts/update-agent-images.sh
# ────────────────────────────────────────────────────────────

{ lib }:

# name: agent name, for the error message
# allowMutableImage: explicit per-agent opt-out (locally-built images have no
#   registry digest, so they cannot be pinned)
# image: the OCI reference to check
name: allowMutableImage: image:
let
  # nixpkgs' oci-containers module splices `container.image` into the container
  # unit's shell command WITHOUT escapeShellArg (unlike every neighbouring
  # element), so this string is a shell-injection sink running as root. Never
  # match it with a leading `.*`: in Nix's POSIX ERE, `.` also matches a
  # newline, a space, `;`, `$` and a backtick, which is enough to smuggle in
  # `--privileged`, a second image, or a command substitution behind a
  # digest-shaped suffix. Whitelist the characters an OCI reference may
  # actually contain, and check it on EVERY path — opting out of
  # reproducibility must not opt out of shell safety.
  isSafeRef = builtins.match "[a-zA-Z0-9][a-zA-Z0-9._/:@-]*" image != null;

  isPinned = builtins.match "[a-zA-Z0-9][a-zA-Z0-9._/:-]*@sha256:[0-9a-f]{64}" image != null;

  # `repo:tag@sha256:...` is valid for the Docker CLI but the containers/image
  # library rejects it ("references with both a tag and digest are currently not
  # supported"), which breaks the podman backend of virtualisation.oci-containers
  # and skopeo. Keep the version in a comment, not in the reference.
  repoPart = builtins.head (lib.splitString "@" image);
  lastSegment = lib.last (lib.splitString "/" repoPart);
  hasTagAndDigest = isPinned && lib.hasInfix ":" lastSegment;

  fail =
    msg:
    builtins.throw ''
      tentaflake: agent "${name}" has an unusable container image:

          ${image}

      ${msg}
    '';
in
# Shell-safety first — this gate applies even with allowMutableImage = true.
if !isSafeRef then
  fail ''
    It contains characters that are not valid in an OCI reference — whitespace,
    a newline, or a shell metacharacter. nixpkgs splices this value unquoted
    into the container unit's shell command, so a reference like this can smuggle
    in extra `docker run` flags or a command substitution that runs as root.

    This is rejected even with `allowMutableImage = true`.
  ''
else if allowMutableImage then
  image
else if hasTagAndDigest then
  fail ''
    It carries both a tag and a digest. The Docker CLI tolerates this, but
    podman and skopeo reject it outright, so the agent would fail to start on a
    podman host. Drop the tag and keep only the digest:

        image = "${lib.removeSuffix ":${lib.last (lib.splitString ":" lastSegment)}" repoPart}@sha256:...";
  ''
else if !isPinned then
  fail ''
    Tags are mutable — the registry can repoint one at different bytes at any
    time, so this build is not reproducible and the running image is not the one
    that was reviewed. Pin it to a digest:

        nix run nixpkgs#skopeo -- inspect --no-tags docker://${image}

    and use the reported Digest:

        image = "<repo>@sha256:<digest>";

    If you deliberately want a mutable or locally-built tag, set
    `allowMutableImage = true;` on this agent to acknowledge that it gives up
    reproducibility.
  ''
else
  image
