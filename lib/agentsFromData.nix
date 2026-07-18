# ────────────────────────────────────────────────────────────
# agentsFromData — turn agents.json (non-Nix, wizard-authored config)
# into the same list of NixOS modules my-agents.nix produces by hand.
#
# agents.json is flat and NON-SECRET (git-tracked): names, model ids,
# providers, ports, and paths to runtime env files holding API keys.
# The keys themselves never appear here — only the `envFile` path that
# points at them.
#
# Usage (see configuration.nix):
#   agentsFromData { file = ./agents.json; inherit mkHermesAgent mkZeroClawAgent; }
# ────────────────────────────────────────────────────────────

{ pkgs, lib }:
{
  file,
  mkHermesAgent,
  mkZeroClawAgent,
}:
let
  data = builtins.fromJSON (builtins.readFile file);

  hermesModule =
    e:
    mkHermesAgent {
      name = e.name;
      envFile = e.envFile;
      settings.model = {
        default = e.model;
        provider = e.provider;
      }
      // lib.optionalAttrs ((e.base_url or null) != null) { base_url = e.base_url; };
    };

  zeroclawModule =
    e:
    mkZeroClawAgent {
      name = e.name;
      agenixFile = e.envFile;
      hostPort = e.hostPort;
      servePort = e.servePort;
      settings = {
        schema_version = 3;
        providers.models.${e.provider}.default = {
          model = e.model;
        }
        // lib.optionalAttrs ((e.base_url or null) != null) { uri = e.base_url; };
        runtime_profiles.default = {
          agentic = true;
          max_tool_iterations = 25;
        };
        agents.main = {
          model_provider = "${e.provider}.default";
          risk_profile = "default";
          runtime_profile = "default";
        };
        risk_profiles.default.level = "supervised";
      };
    };
in
map hermesModule (data.hermes or [ ]) ++ map zeroclawModule (data.zeroclaw or [ ])
