{ mkHermesAgent }:

# ── Default agents for the live ISO ──
# Both use env files at /run/tentaflake/<name>.env (tmpfs, created by firstboot wizard)
# Piper TTS runs as a host service at http://localhost:5001/v1

let
  ttsConfig = {
    provider = "openai";
    openai = {
      base_url = "http://localhost:5001/v1";
      model = "piper";
      voice = "en_US-lessac-medium";
    };
  };

  sttConfig = {
    enabled = true;
    provider = "groq";
    groq.model = "whisper-large-v3-turbo";
  };
in
[
  # ── Agent 1: default (general purpose) ──
  (mkHermesAgent {
    name = "default";
    envFile = "/run/tentaflake/default.env";

    settings = {
      model = {
        default = "deepseek/deepseek-v4-flash";
        provider = "openrouter";
        fallbacks = [
          {
            provider = "openrouter";
            model = "google/gemini-2.5-flash-lite";
          }
          {
            provider = "openrouter";
            model = "deepseek/deepseek-v4-flash";
          }
        ];
      };

      auxiliary = {
        vision.model = "anthropic/claude-haiku-4.5";
        vision.provider = "openrouter";
        compression.model = "deepseek/deepseek-v4-flash";
        compression.provider = "openrouter";
        skills_hub.model = "anthropic/claude-haiku-4.5";
        skills_hub.provider = "openrouter";
        approval.model = "deepseek/deepseek-v4-flash";
        approval.provider = "openrouter";
      };

      compression = {
        enabled = true;
        threshold = 0.50;
        target_ratio = 0.20;
        protect_last_n = 20;
        protect_first_n = 3;
      };

      memory = {
        memory_enabled = true;
        user_profile_enabled = true;
      };

      agent.max_turns = 50;

      display = {
        tool_progress = "off";
        busy_input_mode = "queue";
      };

      terminal.backend = "none";
      web.backend = "firecrawl";

      approvals.mode = "smart";

      toolsets = [
        "terminal"
        "web"
        "memory"
        "file"
        "skills"
      ];

      file_read_max_chars = 100000;

      tool_output = {
        max_bytes = 150000;
        max_lines = 5000;
      };

      providers.openrouter = {
        request_timeout_seconds = 1800;
        stale_timeout_seconds = 300;
      };

      cron.timezone = "UTC";

      inherit ttsConfig sttConfig;
    };

    extraVolumes = [ ];

    extraEnvironment = {
      HERMES_PROFILE = "default";
      HERMES_AGENT_NAME = "default";
    };

    extraContainerConfig = {
      extraOptions = [
        "--network=host"
        "--memory=2g"
      ];
    };
  })

  # ── Agent 2: research (web-focused, deeper research) ──
  (mkHermesAgent {
    name = "research";
    envFile = "/run/tentaflake/research.env";

    settings = {
      model = {
        default = "deepseek/deepseek-v4-flash";
        provider = "openrouter";
        fallbacks = [
          {
            provider = "openrouter";
            model = "google/gemini-2.5-flash-lite";
          }
          {
            provider = "openrouter";
            model = "deepseek/deepseek-v4-flash";
          }
        ];
      };

      auxiliary = {
        vision.model = "anthropic/claude-haiku-4.5";
        vision.provider = "openrouter";
        web_extract.model = "deepseek/deepseek-v4-pro";
        web_extract.provider = "openrouter";
        compression.model = "deepseek/deepseek-v4-flash";
        compression.provider = "openrouter";
        skills_hub.model = "anthropic/claude-haiku-4.5";
        skills_hub.provider = "openrouter";
        approval.model = "deepseek/deepseek-v4-flash";
        approval.provider = "openrouter";
      };

      compression = {
        enabled = true;
        threshold = 0.40;
        target_ratio = 0.15;
        protect_last_n = 20;
        protect_first_n = 3;
      };

      memory = {
        memory_enabled = true;
        user_profile_enabled = true;
      };

      agent.max_turns = 100;

      display = {
        tool_progress = "off";
        busy_input_mode = "queue";
      };

      terminal.backend = "none";
      web.backend = "firecrawl";

      approvals.mode = "smart";

      toolsets = [
        "terminal"
        "web"
        "memory"
        "file"
        "skills"
      ];

      file_read_max_chars = 200000;

      tool_output = {
        max_bytes = 150000;
        max_lines = 5000;
      };

      providers.openrouter = {
        request_timeout_seconds = 1800;
        stale_timeout_seconds = 300;
      };

      cron.timezone = "UTC";

      inherit ttsConfig sttConfig;
    };

    extraVolumes = [ ];

    extraEnvironment = {
      HERMES_PROFILE = "research";
      HERMES_AGENT_NAME = "research";
    };

    extraContainerConfig = {
      extraOptions = [
        "--network=host"
        "--memory=4g"
      ];
    };
  })
]
