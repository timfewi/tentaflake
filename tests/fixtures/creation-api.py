"""Guest-only TLS relay and non-secret probes for the real generated API owner."""
import http.client
import http.server
import json
import os
from pathlib import Path
import secrets
import ssl
import subprocess
import sys

ROOT = Path("/run/tentaflake-creation-api")
HOST = "creation-api.example.invalid"
PORT = 18443
ORIGIN = f"https://{HOST}:{PORT}"
MAX_RESPONSE = 1024 * 1024


def seed():
    ROOT.mkdir(mode=0o700)
    os.umask(0o077)
    (ROOT / "bearer").write_text(secrets.token_hex(32))
    (ROOT / "unsupported-create.json").write_text('{"planId":"fixture","startAfterCreate":false}')
    commands = [
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256", "-days", "2",
         "-keyout", str(ROOT / "ca.key"), "-out", str(ROOT / "ca.pem"), "-subj", "/CN=Creation fixture CA",
         "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign"],
        ["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-sha256",
         "-keyout", str(ROOT / "server.key"), "-out", str(ROOT / "server.csr"), "-subj", f"/CN={HOST}"],
        ["openssl", "x509", "-req", "-in", str(ROOT / "server.csr"), "-CA", str(ROOT / "ca.pem"),
         "-CAkey", str(ROOT / "ca.key"), "-CAcreateserial", "-out", str(ROOT / "server.pem"),
         "-days", "2", "-sha256", "-extfile", str(ROOT / "server.ext")],
    ]
    (ROOT / "server.ext").write_text(
        f"subjectAltName=DNS:{HOST}\nbasicConstraints=critical,CA:FALSE\n"
        "keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n"
    )
    for command in commands:
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    (ROOT / "ca.pem").chmod(0o644)  # Public material; only root may change it.


class Relay(http.server.HTTPServer):
    def __init__(self):
        self.tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.tls.load_cert_chain(ROOT / "server.pem", ROOT / "server.key")
        super().__init__(("127.0.0.1", PORT), Handler)

    def get_request(self):
        stream, address = super().get_request()
        stream.settimeout(15)
        try:
            return self.tls.wrap_socket(stream, server_side=True), address
        except BaseException:
            stream.close()
            raise

    def handle_error(self, request, client_address):
        pass  # Never log credentials, requests or exception objects.


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        self.close_connection = True
        if (self.headers.get_all("Host") != [f"{HOST}:{PORT}"]
                or len(self.path) > 8192 or not self.path.startswith("/api/")
                or self.headers.get("Transfer-Encoding") is not None
                or sum(len(k) + len(v) for k, v in self.headers.items()) > 32768):
            self.send_error(400)
            return
        lengths = self.headers.get_all("Content-Length", ["0"])
        if len(lengths) != 1 or not lengths[0].isdecimal() or int(lengths[0]) > 32768:
            self.send_error(400)
            return
        body = self.rfile.read(int(lengths[0]))
        if len(body) != int(lengths[0]):
            return
        connection = http.client.HTTPConnection("127.0.0.1", 18181, timeout=10)
        try:
            connection.putrequest(self.command, self.path, skip_host=True, skip_accept_encoding=True)
            for key, value in self.headers.items():
                if key.lower() not in ("connection", "content-length"):
                    connection.putheader(key, value)
            connection.putheader("Content-Length", str(len(body)))
            connection.putheader("Connection", "close")
            connection.endheaders(body)
            response = connection.getresponse()
            result = response.read(MAX_RESPONSE + 1)
            if len(result) > MAX_RESPONSE:
                self.send_error(502)
                return
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in ("connection", "content-length", "transfer-encoding", "server", "date"):
                    self.send_header(key, value)
            self.send_header("Content-Length", str(len(result)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(result)
        finally:
            connection.close()


def request(method, target, body=None):
    context = ssl.create_default_context(cafile=str(ROOT / "ca.pem"))
    connection = http.client.HTTPSConnection(HOST, PORT, context=context, timeout=15)
    try:
        token = (ROOT / "bearer").read_text()
        headers = {"Authorization": "Bearer " + token}
        if body is not None:
            headers["Content-Type"] = "application/json"
        connection.request(method, target, body=body, headers=headers)
        reply = connection.getresponse()
        data = reply.read(MAX_RESPONSE + 1)
        assert len(data) <= MAX_RESPONSE
        assert reply.getheader("Cache-Control") == "no-store"
        return {"status": reply.status, "etag": reply.getheader("ETag"), "body": json.loads(data)}
    finally:
        connection.close()


def main():
    if sys.argv[1:] == ["seed"]:
        seed()
    elif sys.argv[1:] == ["relay"]:
        with Relay() as relay:
            relay.serve_forever()
    elif len(sys.argv) == 3 and sys.argv[1] == "operation":
        # IDs are public receipt data; the driver supplies only its exact fixture IDs.
        assert sys.argv[2].startswith("op-") and all(c.isalnum() or c in "-_" for c in sys.argv[2])
        print(json.dumps(request("GET", "/api/v1/operations/" + sys.argv[2])))
    elif sys.argv[1:] == ["unsupported"]:
        response = request("POST", "/api/v1/environments", b'{"planId":"fixture","startAfterCreate":false}')
        assert response["status"] == 501 and response["body"]["error"]["code"] == "unsupported"
    else:
        raise ValueError("unknown creation API fixture phase")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, ValueError, subprocess.SubprocessError, http.client.HTTPException):
        raise SystemExit("creation API fixture failed") from None
