# ────────────────────────────────────────────────────────────
# constants.nix — Template-wide constants
#
# Single source of truth for default values shared across modules.
# Override these via tentaflake.* options in your host configuration.
# ────────────────────────────────────────────────────────────

{
  # NixOS state version for stateful data compatibility. Set once at first install
  # and normally left unchanged across upgrades.
  stateVersion = "26.05";

  # Locale defaults
  defaultLocale = "en_US.UTF-8";
  consoleKeyMap = "us";

  # Host name
  hostName = "agent-host";

  # Default Admin name
  adminUser = "admin";

  # Default shell
  # NOTE: Override via tentaflake.adminShell to ${pkgs.bash}/bin/bash for portability
  adminShell = "/run/current-system/sw/bin/bash";

  # Default user description
  adminDescription = "System Administrator";

  # Agent container images — DIGEST-PINNED, never tags.
  #
  # A tag is a mutable pointer the registry owner can repoint at any time; a
  # digest names exact bytes. mkHermesAgent/mkZeroClawAgent reject unpinned
  # images outright (see lib/pinnedImage.nix). These are multi-arch OCI index
  # digests, so they resolve on both amd64 and arm64.
  #
  # Refresh with ./scripts/update-agent-images.sh
  #
  # Registries are spelled out — podman does not assume docker.io for a bare
  # repository name the way the docker CLI does.
  #
  # docker.io/nousresearch/hermes-agent:latest as of 2026-07-18
  hermesImage = "docker.io/nousresearch/hermes-agent@sha256:4a2f23bd3ffaa6ee7b3be8a302a38be43ab0321a2988cd3fb16b7dd472dde812";
  # ghcr.io/zeroclaw-labs/zeroclaw:v0.8.2
  # (the tag is NOT part of the reference — podman/skopeo reject `repo:tag@digest`)
  zeroclawImage = "ghcr.io/zeroclaw-labs/zeroclaw@sha256:eae321dac2d314bc282bdfb28b5378c9d527998f7e2fe0dee8315bfdcdf13a0c";

  # UID/GID the official hermes-agent image runs its `hermes` user as. The host
  # state dir + any mounted data must be owned by this so the container (which
  # runs as this uid) can write them. Owning host dirs to a *different* uid (e.g.
  # an auto-allocated NixOS system user) is the classic cause of PermissionError
  # on $HERMES_HOME. Override via mkHermesAgent's `containerUid`/`containerGid`.
  containerUid = 10000;
  containerGid = 10000;
}
