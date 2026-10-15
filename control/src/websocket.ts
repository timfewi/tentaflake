// A WebSocket client (ws:// only) for OpenCode's terminal. Bun's WebSocket
// takes a frame of any size whole, and the server in an environment is not
// trusted: this one closes the connection on a frame over its limit before
// reading the frame's payload. Text arrives as it comes, without message
// boundaries; binary messages are skipped and pings are answered.

import { createHash, randomBytes } from "node:crypto";
import { connect, type Socket } from "node:net";

const acceptGuid = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const answerHeadLimit = 8 * 1024;

export class UpstreamSocket {
  private readonly decoder = new TextDecoder();
  // Whether the message in progress is text, for its continuation frames.
  private text = false;
  private state: "open" | "closed" | "failed" = "open";
  private onText: (text: string) => void = () => {};
  private onClose: (failed: boolean) => void = () => {};

  private constructor(
    private readonly socket: Socket,
    private buffer: Buffer,
    private readonly limit: number,
  ) {
    socket.on("data", (chunk: Buffer) => this.receive(chunk));
    socket.on("error", () => this.close());
    socket.on("close", () => this.close());
    socket.pause();
  }

  /** Opens `url` with `headers`; frames wait in the connection until start(). */
  static connect(
    url: string,
    headers: Record<string, string>,
    limit: number,
    timeoutMs: number,
  ): Promise<UpstreamSocket> {
    const { host, hostname, port, pathname, search } = new URL(url);
    const key = randomBytes(16).toString("base64");
    const accept = createHash("sha1").update(`${key}${acceptGuid}`).digest("base64");
    const request = [
      `GET ${pathname}${search} HTTP/1.1`,
      `host: ${host}`,
      "connection: Upgrade",
      "upgrade: websocket",
      "sec-websocket-version: 13",
      `sec-websocket-key: ${key}`,
      ...Object.entries(headers).map(([name, value]) => `${name}: ${value}`),
    ];
    return new Promise((resolve, reject) => {
      const socket = connect({ host: hostname, port: Number(port || 80) });
      let head = Buffer.alloc(0);
      const fail = (reason: string) => {
        clearTimeout(timer);
        socket.destroy();
        reject(new Error(reason));
      };
      const timer = setTimeout(() => fail("no answer"), timeoutMs);
      const onError = (error: Error) => fail(error.message);
      const onClose = () => fail("the connection closed");
      const onData = (chunk: Buffer) => {
        head = Buffer.concat([head, chunk]);
        const end = head.indexOf("\r\n\r\n");
        if (end === -1) {
          if (head.length > answerHeadLimit) fail("the upgrade answer is too long");
          return;
        }
        const [status = "", ...fields] = head.subarray(0, end).toString("latin1").split("\r\n");
        const accepted = fields.some(
          (field) => /^sec-websocket-accept:/i.test(field) && field.slice(21).trim() === accept,
        );
        if (!/^HTTP\/1\.1 101 /.test(status) || !accepted) return fail("the upgrade was refused");
        clearTimeout(timer);
        socket.off("data", onData).off("error", onError).off("close", onClose);
        resolve(new UpstreamSocket(socket, head.subarray(end + 4), limit));
      };
      socket.on("data", onData).on("error", onError).on("close", onClose);
      socket.on("connect", () => socket.write(`${request.join("\r\n")}\r\n\r\n`));
    });
  }

  /** Delivers text from now on; onClose runs once, with whether the server broke the protocol. */
  start(onText: (text: string) => void, onClose: (failed: boolean) => void): void {
    this.onText = onText;
    this.onClose = onClose;
    if (this.state === "open") {
      this.receive(Buffer.alloc(0));
      this.socket.resume();
    } else {
      onClose(this.state === "failed");
    }
  }

  send(text: string): void {
    this.write(0x1, Buffer.from(text));
  }

  close(failed = false): void {
    if (this.state !== "open") return;
    // The close frame goes out without waiting for the answer.
    if (!failed && !this.socket.destroyed) this.write(0x8, Buffer.from([0x03, 0xe8]));
    this.state = failed ? "failed" : "closed";
    this.socket.destroy();
    this.onClose(failed);
  }

  private receive(chunk: Buffer): void {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (this.state === "open" && this.buffer.length >= 2) {
      const frame = this.buffer;
      const second = frame.readUInt8(1);
      const short = second & 0x7f;
      const start = short === 126 ? 4 : short === 127 ? 10 : 2;
      if (frame.length < start) return;
      const length =
        short === 126
          ? frame.readUInt16BE(2)
          : short === 127
            ? Number(frame.readBigUInt64BE(2))
            : short;
      // A server's frames are never masked.
      if (length > this.limit || (second & 0x80) !== 0) {
        this.close(true);
        return;
      }
      if (frame.length < start + length) return;
      this.buffer = frame.subarray(start + length);
      this.frame(frame.readUInt8(0) & 0x0f, frame.subarray(start, start + length));
    }
  }

  private frame(opcode: number, payload: Buffer): void {
    if (opcode <= 0x2) {
      if (opcode !== 0x0) this.text = opcode === 0x1;
      const text = this.text ? this.decoder.decode(payload, { stream: true }) : "";
      if (text !== "") this.onText(text);
    } else if (opcode === 0x8) {
      this.close();
    } else if (opcode === 0x9) {
      this.write(0xa, payload);
    }
  }

  // A client's frame is masked (RFC 6455, section 5.3).
  private write(opcode: number, data: Buffer): void {
    if (this.state !== "open") return;
    const size = data.length < 126 ? 0 : data.length < 0x10000 ? 2 : 8;
    const frame = Buffer.alloc(6 + size + data.length);
    frame.writeUInt8(0x80 | opcode, 0);
    frame.writeUInt8(0x80 | (size === 0 ? data.length : size === 2 ? 126 : 127), 1);
    if (size === 2) frame.writeUInt16BE(data.length, 2);
    if (size === 8) frame.writeBigUInt64BE(BigInt(data.length), 2);
    const mask = randomBytes(4);
    mask.copy(frame, 2 + size);
    for (let index = 0; index < data.length; index++) {
      frame[6 + size + index] = (data[index] ?? 0) ^ (mask[index & 3] ?? 0);
    }
    this.socket.write(frame);
  }
}
