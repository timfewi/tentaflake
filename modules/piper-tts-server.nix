{
  config,
  lib,
  pkgs,
  ...
}:

# ────────────────────────────────────────────────────────────
# Piper TTS HTTP Server — OpenAI-compatible /v1/audio/speech
#
# Serves a local TTS endpoint so Hermes agents (via host
# networking) can use Piper as their TTS provider.
#
# Usage:
#   services.piper-tts-server = {
#     enable    = true;
#     voiceName = "en_US-lessac-medium";
#     voiceModel = pkgs.fetchurl {
#       url = "https://.../voice.onnx";
#       hash = "...";
#     };
#     voiceConfig = pkgs.fetchurl {
#       url = "https://.../voice.onnx.json";
#       hash = "...";
#     };
#   };
#
# Hermes config.yaml to use this:
#   tts:
#     provider: openai
#     openai:
#       base_url: "http://localhost:5001/v1"
#       model: "piper"
#       voice: "de_DE-thorsten-medium"
# ────────────────────────────────────────────────────────────

let
  cfg = config.services.piper-tts-server;

  # Tiny Python HTTP server using only stdlib
  serverPy = pkgs.writeText "piper-tts-server.py" ''
    import http.server
    import json
    import os
    import subprocess
    import io
    import wave
    import struct
    import sys

    MODEL = os.environ.get("PIPER_MODEL", "")
    CONFIG = os.environ.get("PIPER_CONFIG", "")
    PORT = int(os.environ.get("PIPER_PORT", "5001"))
    HOST = os.environ.get("PIPER_HOST", "127.0.0.1")

    class TTSHandler(http.server.BaseHTTPRequestHandler):
        def do_OPTIONS(self):
            self.send_response(200)
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Access-Control-Allow-Methods", "POST, OPTIONS")
            self.send_header("Access-Control-Allow-Headers", "Content-Type")
            self.end_headers()

        def do_GET(self):
            if self.path == "/health":
                self._json(200, {"status": "ok", "model": MODEL, "port": PORT})
            elif self.path in ("/v1/models", "/models"):
                self._json(200, {
                    "object": "list",
                    "data": [{
                        "id": "piper",
                        "object": "model",
                        "created": 0,
                        "owned_by": "piper-tts",
                    }]
                })
            else:
                self._json(404, {"error": "not found"})

        def do_POST(self):
            if self.path == "/v1/audio/speech":
                self._handle_tts()
            else:
                self._json(404, {"error": "not found"})

        def _handle_tts(self):
            content_len = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_len)
            try:
                data = json.loads(body)
            except json.JSONDecodeError:
                self._json(400, {"error": "invalid JSON"})
                return

            text = data.get("input", "")
            if not text:
                self._json(400, {"error": "input text required"})
                return

            try:
                proc = subprocess.run(
                    ["piper",
                     "--model", MODEL,
                     "--config", CONFIG,
                     "--output-raw"],
                    input=text.encode("utf-8"),
                    capture_output=True,
                    timeout=60,
                )
            except FileNotFoundError:
                self._json(500, {"error": "piper binary not found"})
                return
            except subprocess.TimeoutExpired:
                self._json(500, {"error": "piper timed out"})
                return

            if proc.returncode != 0:
                self._json(500, {"error": "piper failed", "stderr": proc.stderr.decode(errors="replace")})
                return

            raw_pcm = proc.stdout

            # Determine sample rate from config or default to 22050
            sample_rate = 22050
            try:
                with open(CONFIG) as f:
                    cfg_data = json.load(f)
                    sample_rate = cfg_data.get("audio", {}).get("sample_rate", 22050)
            except Exception:
                pass

            # Wrap raw S16_LE PCM in WAV header
            buf = io.BytesIO()
            with wave.open(buf, "wb") as wav:
                wav.setnchannels(1)
                wav.setsampwidth(2)  # 16-bit
                wav.setframerate(sample_rate)
                wav.writeframes(raw_pcm)
            wav_data = buf.getvalue()

            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Content-Length", str(len(wav_data)))
            self.end_headers()
            self.wfile.write(wav_data)

        def _json(self, status, data):
            body = json.dumps(data).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, fmt, *args):
            pass  # systemd/journald handles logging

    if not MODEL or not CONFIG:
        print("FATAL: PIPER_MODEL and PIPER_CONFIG must be set", file=sys.stderr)
        sys.exit(1)

    print(f"Piper TTS server starting on {HOST}:{PORT}")
    print(f"  Model:  {MODEL}")
    print(f"  Config: {CONFIG}")
    server = http.server.HTTPServer((HOST, PORT), TTSHandler)
    server.serve_forever()
  '';

in
{
  options.services.piper-tts-server = {
    enable = lib.mkEnableOption "Piper TTS HTTP server";

    voiceName = lib.mkOption {
      type = lib.types.str;
      default = "voice";
      description = "Voice display name (used in logs and model list)";
    };

    voiceModel = lib.mkOption {
      type = lib.types.path;
      description = "Path to Piper .onnx voice model file";
    };

    voiceConfig = lib.mkOption {
      type = lib.types.path;
      description = "Path to Piper .onnx.json voice config file";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 5001;
      description = "HTTP server port (localhost only)";
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Bind address (default localhost only)";
    };
  };

  config = lib.mkIf cfg.enable {
    # Install piper-tts system-wide
    environment.systemPackages = [ pkgs.piper-tts ];

    # Dedicated system user
    users.users.piper-tts = {
      isSystemUser = true;
      group = "piper-tts";
      description = "Piper TTS service user";
    };
    users.groups.piper-tts = { };

    # Systemd service
    systemd.services.piper-tts = {
      description = "Piper TTS HTTP server (OpenAI-compatible /v1/audio/speech)";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        ExecStart = "${pkgs.python3}/bin/python3 ${serverPy}";
        User = "piper-tts";
        Restart = "always";
        RestartSec = "3";
        PrivateTmp = true;
        NoNewPrivileges = true;
      };

      environment = {
        PIPER_MODEL = toString cfg.voiceModel;
        PIPER_CONFIG = toString cfg.voiceConfig;
        PIPER_PORT = toString cfg.port;
        PIPER_HOST = cfg.host;
      };
    };
  };
}
