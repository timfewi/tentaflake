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

{ lib, ... }:
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
      inherit (e) name;
      inherit (e) envFile;
      settings.model = {
        default = e.model;
        inherit (e) provider;
      }
      // lib.optionalAttrs ((e.base_url or null) != null) { inherit (e) base_url; };
    };

  zeroclawModule =
    e:
    mkZeroClawAgent {
      inherit (e) name;
      agenixFile = e.envFile;
      inherit (e) hostPort;
      inherit (e) servePort;
      settings = {
        schema_version = 3;
        providers.models.${e.provider}.default = {
          inherit (e) model;
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
