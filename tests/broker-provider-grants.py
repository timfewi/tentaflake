"""Exercise the packaged broker with independent loopback origins and disposable credentials."""

import concurrent.futures
import contextlib
import http.client
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time


PROTOCOLS = {
    "openai": ("openai-responses", "/v1/responses", "/v1/responses"),
    "anthropic": ("anthropic-messages", "/v1/messages", "/v1/messages"),
    "opencode-go": (
        "opencode-go-chat-completions",
        "/v1/chat/completions",
        "/inference/go/openai/v1/chat/completions",
    ),
}
TOOL = "research_read"


def response(provider, model):
    usage = {"input_tokens": 7, "output_tokens": 3}
    if provider == "anthropic":
        return {
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "id": "response-1",
            "model": model,
            "stop_reason": "end_turn",
            "usage": usage,
        }
    if provider == "openai":
        return {
            "id": "response-1",
            "model": model,
            "object": "response",
            "status": "completed",
            "output": [],
            "usage": {**usage, "total_tokens": 10},
        }
    return {
        "id": "response-1",
        "object": "chat.completion",
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop",
            }
        ],
        "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10},
    }


def stream(provider, model):
    reply = response(provider, model)
    if provider == "anthropic":
        events = [
            {
                "type": "message_start",
                "message": {
                    **reply,
                    "content": [],
                    "stop_reason": None,
                    "usage": {"input_tokens": 7, "output_tokens": 0},
                },
            },
            {
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""},
            },
            {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "ok"},
            },
            {"type": "content_block_stop", "index": 0},
            {
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 3},
            },
            {"type": "message_stop"},
        ]
    elif provider == "openai":
        events = [{"type": "response.completed", "response": reply}]
    else:
        chunk = {"id": reply["id"], "object": "chat.completion.chunk", "model": model}
        events = [
            {
                **chunk,
                "choices": [
                    {"index": 0, "delta": {"content": "ok"}, "finish_reason": "stop"}
                ],
            },
            {**chunk, "choices": [], "usage": reply["usage"]},
            "[DONE]",
        ]
    return "".join(
        "data: " + (event if isinstance(event, str) else json.dumps(event)) + "\n\n"
        for event in events
    ).encode()


def fixture(provider):
    class Origin(BaseHTTPRequestHandler):
        def do_POST(self):
            payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            observations = self.server.observations
            observations.append((self.path, dict(self.headers.items()), payload))
            model = payload["model"]
            data = (
                stream(provider, model)
                if payload.get("stream")
                else json.dumps(response(provider, model)).encode()
            )
            if self.server.mode == "reject":
                data = b'{"error":"private upstream diagnostic test-only-anthropic"}'
                self.send_response(429)
            elif self.server.mode == "incomplete":
                data = b'data: {"type":"response.created"}\n\n'
                self.send_response(200)
            else:
                self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.send_header(
                "Content-Type",
                "text/event-stream" if payload.get("stream") else "application/json",
            )
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    return Origin


def body(provider, tool=TOOL):
    model = f"{provider}-fixture-model"
    if provider == "anthropic":
        return {
            "model": model,
            "messages": [{"role": "user", "content": "sample"}],
            "max_tokens": 8,
            "tools": [{"name": tool, "input_schema": {"type": "object"}}],
        }
    if provider == "openai":
        return {
            "model": model,
            "input": "sample",
            "max_output_tokens": 8,
            "tools": [
                {"type": "function", "name": tool, "parameters": {"type": "object"}}
            ],
        }
    return {
        "model": model,
        "messages": [{"role": "user", "content": "sample"}],
        "max_tokens": 8,
        "tools": [
            {
                "type": "function",
                "function": {"name": tool, "parameters": {"type": "object"}},
            }
        ],
    }


@contextlib.contextmanager
def running_broker(binary, directory, config, port):
    path = directory / "config.json"
    path.write_text(json.dumps(config))
    with subprocess.Popen(
        [binary, "--config", str(path)], stderr=subprocess.PIPE, text=True
    ) as process:
        try:
            for _ in range(200):
                if process.poll() is not None:
                    raise RuntimeError(process.stderr.read())
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                        break
                except OSError:
                    time.sleep(0.025)
            else:
                raise AssertionError("broker did not start")
            yield
        finally:
            process.terminate()
            process.wait(timeout=4)


def main(binary):
    with tempfile.TemporaryDirectory() as tmp, contextlib.ExitStack() as stack:
        directory = Path(tmp)

        def credential(name):
            path = directory / name
            path.write_text("test-only-" + name)
            path.chmod(0o600)
            return str(path)

        virtual = credential("capsule")
        grants, origins = [], {}
        for provider, (protocol, _, upstream) in PROTOCOLS.items():
            origin = stack.enter_context(
                ThreadingHTTPServer(("127.0.0.1", 0), fixture(provider))
            )
            origin.observations = []
            origin.mode = "normal"
            threading.Thread(target=origin.serve_forever, daemon=True).start()
            stack.callback(origin.shutdown)
            origins[provider] = origin
            grants.append(
                {
                    "version": 1,
                    "revision": "grant-" + provider,
                    "provider": provider,
                    "protocol": protocol,
                    "upstream_endpoint": f"http://127.0.0.1:{origin.server_port}{upstream}",
                    "provider_credential_file": credential(provider),
                    "model": {
                        "name": provider + "-fixture-model",
                        "input_microusd_per_million": 1_000_000,
                        "output_microusd_per_million": 2_000_000,
                    },
                    "pricing_revision": "price-1",
                    "currency": "USD",
                    "tools": [TOOL],
                    "max_output_tokens": 8,
                }
            )
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        config = {
            "agent": "fixture",
            "listen": f"127.0.0.1:{port}",
            "token_file": virtual,
            "audit_file": str(directory / "audit"),
            "budget_state_file": str(directory / "budget"),
            "max_requests_per_window": 120,
            "llm": {
                "grants": grants,
                "allow_plain_http_for_tests": True,
                "streaming": {"enable": True},
            },
        }

        def call(
            provider,
            operation,
            payload=None,
            *,
            route=None,
            revision=None,
            session="fixture-session",
            bearer="test-only-capsule",
        ):
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
            headers = {
                "Content-Type": "application/json",
                "Authorization": "Bearer " + bearer,
                "x-tentaflake-grant": revision or "grant-" + provider,
                "x-tentaflake-operation": operation,
                "x-api-key": "untrusted-client-key",
            }
            if provider == "opencode-go" and session:
                headers["x-opencode-session"] = session
            try:
                conn.request(
                    "POST",
                    route or PROTOCOLS[provider][1],
                    json.dumps(payload or body(provider)),
                    headers,
                )
                result = conn.getresponse()
                return result.status, result.read()
            finally:
                conn.close()

        with running_broker(binary, directory, config, port):
            for provider, origin in origins.items():
                for streaming in [False, True]:
                    operation = f"{provider}-{streaming}"
                    payload = {**body(provider), "stream": streaming}
                    status, data = call(provider, operation, payload)
                    assert status == 200, (provider, status, data)
                    if streaming:
                        assert data == stream(provider, payload["model"]), (
                            provider,
                            data,
                        )
                    else:
                        assert json.loads(data) == response(provider, payload["model"])
                    path, raw_headers, actual = origin.observations[-1]
                    headers = {key.lower(): value for key, value in raw_headers.items()}
                    assert actual == payload and path == PROTOCOLS[provider][2]
                    assert (
                        "x-tentaflake-grant" not in headers
                        and "x-tentaflake-operation" not in headers
                    )
                    if provider == "anthropic":
                        assert (
                            headers["x-api-key"] == "test-only-anthropic"
                            and "authorization" not in headers
                        )
                        assert headers["anthropic-version"] == "2023-06-01"
                    else:
                        assert (
                            headers["authorization"] == "Bearer test-only-" + provider
                            and "x-api-key" not in headers
                        )
                    assert headers.get("x-opencode-session") == (
                        "fixture-session" if provider == "opencode-go" else None
                    )
                assert len(origin.observations) == 2
                for kwargs in [
                    {"payload": body(provider, "research_admin")},
                    {"revision": "undeclared"},
                    {"payload": {**body(provider), "fallbacks": ["alternate-model"]}},
                    {"payload": {**body(provider), "model": "wrong"}},
                    {"bearer": "test-only-" + provider},
                    {
                        "payload": {
                            **body(provider),
                            (
                                "max_output_tokens"
                                if provider == "openai"
                                else "max_tokens"
                            ): 9,
                        }
                    },
                    {
                        "route": "/v1/messages"
                        if provider == "openai"
                        else "/v1/responses"
                    },
                ]:
                    assert 400 <= call(provider, "bad", **kwargs)[0] < 500
                if provider == "opencode-go":
                    assert call(provider, "bad", session=None)[0] == 400
                assert len(origin.observations) == 2
            with concurrent.futures.ThreadPoolExecutor(2) as pool:
                results = list(
                    pool.map(lambda _: call("openai", "concurrent")[0], range(2))
                )
            assert results.count(200) == 1 and all(
                s in (200, 409, 503) for s in results
            )
            assert len(origins["openai"].observations) == 3
            denied = {**body("anthropic"), "cache_control": {"type": "ephemeral"}}
            assert call("anthropic", "no-cache", denied)[0] == 403
            assert len(origins["anthropic"].observations) == 2
            origins["anthropic"].mode = "reject"
            status, data = call("anthropic", "reject")
            assert status == 429 and b"test-only-" not in data
            origins["openai"].mode = "incomplete"
            try:
                call("openai", "interrupted", {**body("openai"), "stream": True})
            except http.client.IncompleteRead as error:
                assert b"response.completed" not in error.partial
            else:
                raise AssertionError("truncated provider stream ended normally")
            assert call("openai", "interrupted")[0] == 409
        with running_broker(binary, directory, config, port):
            assert call("anthropic", "concurrent")[0] == 409
            assert len(origins["anthropic"].observations) == 3
        raw = (directory / "audit").read_text()
        assert "test-only-" not in raw and "untrusted-client-key" not in raw
        events = [json.loads(line) for line in raw.splitlines()]
        assert any(
            event["outcome"] == "stream_aborted"
            and event["detail"].get("operation") == "interrupted"
            for event in events
        )
        assert any(
            event["outcome"] == "failed"
            and event["detail"].get("operation") == "reject"
            for event in events
        )
        measured = [
            event["detail"] for event in events if event["outcome"] == "completed"
        ]
        assert len(measured) == 7, measured
        for result in measured:
            assert (
                result["measured_cost_microusd"] == 13
                and result["usage"]["input_tokens"] == 7
            )
            assert result["usage"]["output_tokens"] == 3 and result[
                "grant_revision"
            ].startswith("grant-")
        print(
            "Three isolated providers passed JSON and SSE, measured 7 input + 3 output tokens, 13 micro-USD per call. Duplicate after restart produced zero dispatches."
        )


if __name__ == "__main__":
    main(sys.argv[1])
