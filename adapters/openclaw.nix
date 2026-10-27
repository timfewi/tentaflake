# Investigated upstream v2026.9.7. This is deliberately a stopped scaffold:
# no reviewed runtime artifact or accepted model/Research/execution routing.
# See docs/agent-adapters.md before adding an OCI image or enabling startup.
{ lib, ... }:
import ../lib/stoppedScaffold.nix { inherit lib; } {
  id = "openclaw";
  displayName = "OpenClaw";
  reason = "OpenClaw is a stopped scaffold. The LLM broker supports opt-in streaming, but acceptance with the pinned OpenClaw executable is unverified. A reviewed image digest, accepted model transport, actual Research discovery, and disposable-worker tool routing require acceptance evidence. See docs/agent-adapters.md.";
  settingsReason = "OpenClaw settings are unavailable until the pinned upstream parser and tool controls pass acceptance.";
}
