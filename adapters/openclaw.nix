# Investigated upstream v2026.9.7. This is deliberately a stopped scaffold:
# no reviewed runtime artifact or accepted model/Research/execution routing.
# See docs/agent-adapters.md before adding an OCI image or enabling startup.
{ lib, ... }:
import ../lib/stoppedScaffold.nix { inherit lib; } {
  preset = {
    schemaVersion = 1;
    identity = {
      id = "openclaw";
      version = "2026.9.7";
      status = "scaffold";
    };
    artifact = {
      kind = "unselected";
      reference = null;
      reviewed = false;
    };
    command = [
      "node"
      "openclaw.mjs"
      "gateway"
    ];
    configuration = {
      format = "json";
      readOnly = true;
    };
    ownership = {
      uid = 1000;
      gid = 1000;
    };
    layout = {
      state = "/var/lib/tentaflake/agents/<name>";
      workspace = "workspace";
      writable = [
        "state"
        "workspace"
      ];
      directories = [ "workspace" ];
    };
    lifecycle = "stopped-scaffold";
    model = {
      protocols = [
        "openai-chat-completions"
        "openai-responses"
      ];
      streaming = "required";
    };
    research = {
      supported = false;
    };
    execution = {
      interface = "unverified";
      automatic = false;
    };
    capabilities = [
      "files"
      "shell"
      "terminal"
      "worker"
    ];
    evidence = {
      fixture = "refusal-regression";
      vendorAcceptance = "pending";
    };
  };
  id = "openclaw";
  displayName = "OpenClaw";
  reason = "OpenClaw is a stopped scaffold. The LLM broker supports opt-in streaming, but acceptance with the pinned OpenClaw executable is unverified. A reviewed image digest, accepted model transport, actual Research discovery, and disposable-worker tool routing require acceptance evidence. See docs/agent-adapters.md.";
  settingsReason = "OpenClaw settings are unavailable until the pinned upstream parser and tool controls pass acceptance.";
}
