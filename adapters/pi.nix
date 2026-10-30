# Pi Durable is deliberately a stopped scaffold: no reviewed runtime image,
# accepted runner, host socket boundary or live provider admission exists.
{ lib, ... }:
import ../lib/stoppedScaffold.nix { inherit lib; } {
  preset = {
    schemaVersion = 1;
    identity = {
      id = "pi";
      version = "1.4.0";
      status = "scaffold";
    };
    artifact = {
      kind = "unselected";
      reference = null;
      reviewed = false;
    };
    command = [ ];
    configuration = {
      format = "none";
      readOnly = true;
    };
    ownership = {
      uid = 10100;
      gid = 10100;
    };
    layout = {
      state = "/var/lib/tentaflake/agents/<name>";
      workspace = "workspace";
      writable = [ ];
      directories = [ ];
    };
    lifecycle = "stopped-scaffold";
    model = {
      protocols = [ ];
      streaming = "unknown";
    };
    research = {
      supported = false;
    };
    execution = {
      interface = "unverified";
      automatic = false;
    };
    capabilities = [ ];
    evidence = {
      fixture = "refusal-regression";
      vendorAcceptance = "pending";
    };
  };
  id = "pi";
  displayName = "Pi";
  reason = "Pi is a stopped scaffold. A reviewed image, runner, Board and Research socket boundaries, broker compatibility, and lifecycle acceptance evidence are required before admission. See docs/agent-adapters.md.";
  settingsReason = "Pi settings are unavailable until runtime acceptance.";
}
