"""Local VM fixture: no external calls or real credentials."""

import json
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        if length > 1024 * 1024:
            self.send_error(413)
            return
        body = json.loads(self.rfile.read(length))
        if self.headers.get("Authorization") != "Bearer fixture-provider-key":
            self.send_error(401)
            return
        if not body.get("stream"):
            payload = b'{"id":"completion-fixture"}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        if self.path.endswith("/responses"):
            self.event({"type": "response.output_text.delta", "delta": "stream-first"})
            self.event({"type": "response.completed", "response": {
                "id": "fixture", "status": "completed", "usage": {
                    "input_tokens": 1, "output_tokens": 1, "total_tokens": 2
                }
            }})
        else:
            self.event({"object": "chat.completion.chunk", "choices": [{
                "index": 0, "delta": {"content": "stream-first"}, "finish_reason": None
            }]})
            self.event({"object": "chat.completion.chunk", "choices": [{
                "index": 0, "delta": {}, "finish_reason": "stop"
            }]})
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()

    def event(self, payload):
        self.wfile.write(("data: " + json.dumps(payload) + "\n\n").encode())
        self.wfile.flush()


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", 18080), Handler).serve_forever()
