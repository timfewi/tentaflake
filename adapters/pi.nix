# Pi Durable is deliberately a stopped scaffold: no reviewed runtime image,
# accepted runner, host socket boundary or live provider admission exists.
{ lib, ... }:
import ../lib/stoppedScaffold.nix { inherit lib; } {
  id = "pi";
  displayName = "Pi";
  reason = "Pi is a stopped scaffold. A reviewed image, runner, Board and Research socket boundaries, broker compatibility, and lifecycle acceptance evidence are required before admission. See docs/agent-adapters.md.";
  settingsReason = "Pi settings are unavailable until runtime acceptance.";
}
