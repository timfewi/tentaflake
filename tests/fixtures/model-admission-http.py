"""VM-only provider and echo controls; no external calls or real credentials."""

import runpy
import sys
from http.server import HTTPServer


class Handler(runpy.run_path(sys.argv[1])["Handler"]):
    def do_GET(self):
        payload = b"model-admission-http-control"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


HTTPServer((sys.argv[2], int(sys.argv[3])), Handler).serve_forever()
