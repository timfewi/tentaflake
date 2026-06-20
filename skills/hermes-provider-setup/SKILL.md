---
name: hermes-provider-setup
description: Configure AI providers for Hermes — Nous Portal, OpenRouter, Anthropic, OpenAI, custom endpoints, multi-provider fallback, credential pools
version: 1.0.0
platforms: [linux, macos, windows]
metadata:
  hermes:
    tags: [provider, model, setup, openrouter, anthropic, nous-portal]
    category: devops
    requires_toolsets: [terminal]
---

# Hermes Provider Setup

## When to Use

- Configure LLM provider for Hermes (first-time setup)
- Switch between providers
- Add multi-provider fallback
- Set up custom/self-hosted endpoint (Ollama, vLLM, SGLang)
- Configure credential pool for multi-key rotation
- Troubleshoot provider connectivity
- Switch models mid-session

## Procedure

### 1. Provider Selection Guide

| Provider             | Auth                | Best for                                                          |
| -------------------- | ------------------- | ----------------------------------------------------------------- |
| **Nous Portal**      | OAuth (browser)     | All-in-one: 300+ models + Tool Gateway (web, image, TTS, browser) |
| **OpenAI Codex**     | OAuth (device code) | Codex models, ChatGPT subscribers                                 |
| **Anthropic**        | OAuth or API key    | Claude models (Max plan + credits)                                |
| **OpenRouter**       | API key             | Multi-provider routing, 200+ models                               |
| **Google AI Studio** | API key             | Gemini models                                                     |
| **Custom Endpoint**  | API key + base URL  | Self-hosted (Ollama, vLLM, SGLang, LM Studio)                     |
| **DeepSeek**         | API key             | DeepSeek models direct                                            |
| **AWS Bedrock**      | IAM role            | Claude, Nova within AWS                                           |
| **GitHub Copilot**   | OAuth               | Copilot subscription models                                       |
| **xAI**              | API key or OAuth    | Grok models                                                       |
| **z.ai / ZhipuAI**   | API key             | GLM models                                                        |
| **Kimi / Moonshot**  | API key             | Kimi models                                                       |
| **MiniMax**          | API key or OAuth    | MiniMax models                                                    |

**Minimum context: 64K tokens.** Models with smaller windows rejected at startup.

**Rule of thumb:** If Hermes can't complete a normal chat, don't add features yet. Get one clean conversation working.

### 2. Fastest Path — Nous Portal

```bash
hermes setup --portal
```

This one command:

1. Opens browser for OAuth login (portal.nousresearch.com)
2. Stores refresh token at `~/.hermes/auth.json`
3. Lets you pick a model
4. Sets Nous as inference provider
5. Turns on Tool Gateway (web, image, TTS, browser)
6. Ready to `hermes chat`

After setup, config looks like:

```yaml
model:
  provider: nous
  default: anthropic/claude-sonnet-4.6
  base_url: https://inference-api.nousresearch.com/v1
```

### 3. API Key Providers

```bash
# OpenRouter — most flexible multi-provider
hermes config set OPENROUTER_API_KEY sk-or-...

# Anthropic — direct Claude access
hermes config set ANTHROPIC_API_KEY sk-ant-...

# OpenAI
hermes config set OPENAI_API_KEY sk-...

# Google Gemini
hermes config set GOOGLE_API_KEY AIza...
# or GEMINI_API_KEY

# DeepSeek
hermes config set DEEPSEEK_API_KEY sk-...

# xAI (Grok)
hermes config set XAI_API_KEY ...

# AWS Bedrock — uses IAM role or aws configure
# No API key needed

# GitHub Copilot
hermes config set COPILOT_GITHUB_TOKEN ghp_...
```

### 4. OAuth Providers

```bash
# Nous Portal
hermes portal                        # One-shot OAuth setup
hermes portal info                   # Check login status
hermes portal tools                  # Tool Gateway catalog

# Anthropic OAuth
hermes auth add anthropic --type oauth

# OpenAI Codex
hermes model → pick OpenAI Codex (device code flow)

# Google Gemini OAuth
hermes model → pick Google Gemini (OAuth)

# GitHub Copilot OAuth
hermes model → pick GitHub Copilot (OAuth)

# MiniMax OAuth
hermes model → pick MiniMax (OAuth)

# xAI Grok OAuth
hermes model → pick xAI Grok OAuth

# Qwen OAuth
hermes model → pick Qwen OAuth
```

### 5. Custom Endpoint Configuration

For local/self-hosted models (Ollama, vLLM, SGLang, LM Studio, llama.cpp):

```bash
# Interactive
hermes model
# → Select: Custom endpoint
# → Enter base URL: http://localhost:11434/v1
# → Enter API key: ollama (or empty)
# → Enter model name: qwen3.5:27b
# → Context length: 64000
```

Or direct config:

```yaml
model:
  default: qwen3.5:27b
  provider: custom
  base_url: http://localhost:11434/v1
```

With per-model config:

```yaml
custom_providers:
  - name: "My Ollama"
    base_url: "http://localhost:11434/v1"
    models:
      qwen3.5:27b:
        context_length: 64000
      llama3.2:3b:
        context_length: 8192
```

**Works with:** Ollama, vLLM, SGLang, LM Studio, LocalAI, llama.cpp server, any OpenAI-compatible API.

### 6. Multi-Provider Fallback

```yaml
model:
  provider: openrouter
  default: anthropic/claude-sonnet-4.6
  fallbacks:
    - provider: nous
      model: anthropic/claude-sonnet-4.6
    - provider: anthropic
      model: claude-sonnet-4-20250514
```

Fallback chain engages on errors (rate limits, 5xx, connection drops). `agent.api_max_retries` controls retries before fallback:

```yaml
agent:
  api_max_retries: 3 # Default: 3 retries per provider before fallback
```

Set to `0` for fast failover (don't retry flaky endpoint, switch immediately).

### 7. Provider Switching

```bash
hermes model              # Full setup wizard — add/change providers
```

Inside session:

```text
/model                         # Interactive picker
/model anthropic/claude-sonnet-4.6     # Switch model
/model openrouter:anthropic/claude-sonnet-4.6  # Switch provider + model

# Useful patterns:
/model google/gemini-3-pro-preview     # Large context for long docs
/model openai/gpt-5.4                  # Strong reasoning
/model deepseek/deepseek-v4-pro        # Cost-effective coding
```

**`/model` only shows providers already configured.** Add new ones with `hermes model` from terminal.

### 8. Credential Pool Strategies

Multiple API keys for same provider:

```yaml
credential_pool_strategies:
  openrouter: round_robin # Cycle through keys evenly
  anthropic: least_used # Always pick least-used key
  custom: fill_first # Use first key until exhausted
```

| Strategy               | Behavior                               |
| ---------------------- | -------------------------------------- |
| `fill_first` (default) | Use first key, fall to next on failure |
| `round_robin`          | Cycle keys evenly                      |
| `least_used`           | Always pick least-used                 |
| `random`               | Random selection                       |

### 9. Provider Timeouts

```yaml
providers:
  openrouter:
    request_timeout_seconds: 1800 # API call timeout
    stale_timeout_seconds: 300 # Stale non-stream detection
    models:
      claude-sonnet-4:
        timeout_seconds: 3600 # Model-specific override
        stale_timeout_seconds: 600
```

| Timeout          | Default | Config                                   |
| ---------------- | ------- | ---------------------------------------- |
| Socket read      | 120s    | `HERMES_STREAM_READ_TIMEOUT`             |
| Stale stream     | 180s    | `HERMES_STREAM_STALE_TIMEOUT`            |
| Stale non-stream | 300s    | `providers.<id>.stale_timeout_seconds`   |
| API call         | 1800s   | `providers.<id>.request_timeout_seconds` |

**Local model detection**: Hermes auto-raises read timeout to 1800s, disables stale detection for local endpoints.

### 10. Model Context Length Requirements

Minimum: **64,000 tokens**. Set explicitly if auto-detect wrong:

```yaml
model:
  default: your-model-name
  context_length: 131072
```

For local models, ensure server configured for enough context:

```bash
# llama.cpp
--ctx-size 65536

# Ollama
ollama run --num_ctx 64000
```

### 11. Compression Model Config

Summarization can use different (cheaper) model:

```yaml
auxiliary:
  compression:
    provider: openrouter
    model: google/gemini-3-flash-preview
    base_url: null
```

**Must have context window ≥ main model's.** The compressor sends full middle section to summary model — smaller window causes silent context loss.

### 12. Provider Config Storage

All secrets → `~/.hermes/.env`:

```bash
OPENROUTER_API_KEY=sk-or-...
ANTHROPIC_API_KEY=sk-ant-...
GOOGLE_API_KEY=AIza...
```

All non-secrets → `~/.hermes/config.yaml`:

```yaml
model:
  provider: openrouter
  default: anthropic/claude-sonnet-4.6
```

`hermes config set KEY VAL` auto-routes to correct file.

### 13. Provider Verification

```bash
# Test basic chat
hermes chat -q "hello"

# Test with specific provider
hermes chat --provider openrouter -q "hello"

# Test with specific model
hermes chat --model anthropic/claude-sonnet-4.6 -q "hello"

# Check provider config
hermes config show | head -20

# Full diagnostics
hermes doctor
```

## Pitfalls

- **Model 400 = name mismatch or plan issue**: Verify exact slug with provider docs
- **`/model` only shows configured providers**: Add new providers via `hermes model` from terminal
- **Minimum 64K context**: Models with smaller windows rejected at startup
- **Local model timeouts**: Auto-detected and relaxed. Set `HERMES_STREAM_READ_TIMEOUT=1800` if needed
- **OAuth on headless needs workaround**: Use paste-back or SSH port forward
- **API key conflicts**: OpenAI key won't work with OpenRouter. Check `.env` for conflicting entries
- **Context length auto-detected may be wrong**: Set `model.context_length` explicitly for custom endpoints
- **Compression model needs ≥ main model context**: Smaller window causes silent context loss
- **Fallback too aggressive can cause problems**: Keep routing off until base provider is stable
- **`agent.api_max_retries 0` skips retries**: Fast failover means no retries on transient errors
- **`provider: custom` is first-class provider**: Not an alias. Set `provider: custom` in config.yaml
- **Portal refresh token quarantine**: On invalidation, next call shows "re-authentication required". Run `hermes auth add nous`

## Verification

```bash
hermes doctor                              # Check provider connectivity
hermes chat -q "hello"                     # Basic chat works
hermes config show | grep -A 10 model       # Provider/model config
hermes portal info                         # (if using Portal)
# In session:
/model                                     # Should show configured providers
```
