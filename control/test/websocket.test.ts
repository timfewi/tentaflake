import { afterEach, describe, expect, test } from "bun:test";
import type { Socket } from "bun";
import { createHash } from "node:crypto";
import { UpstreamSocket } from "../src/websocket";
import { until } from "./helpers";

const limit = 1000;
const servers: { stop(force: boolean): void }[] = [];
afterEach(() => {
  for (const server of servers.splice(0)) server.stop(true);
});

const acceptFor = (key: string) =>
  createHash("sha1").update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
const upgraded = (key: string) =>
  `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${acceptFor(key)}\r\n\r\n`;

/** An unmasked server frame. */
function frame(opcode: number, payload: string | Buffer, fin = true): Buffer {
  const data = Buffer.from(payload);
  const head = data.length < 126 ? [data.length] : [126, data.length >> 8, data.length & 0xff];
  return Buffer.from([(fin ? 0x80 : 0) | opcode, ...head, ...data]);
}

/** The client's frames, unmasked. */
function clientFrames(bytes: Buffer): { opcode: number; masked: boolean; payload: Buffer }[] {
  const frames = [];
  for (let at = 0; at < bytes.length; ) {
    const short = (bytes[at + 1] ?? 0) & 0x7f;
    const start = at + (short === 126 ? 4 : short === 127 ? 10 : 2);
    const length =
      short === 126
        ? bytes.readUInt16BE(at + 2)
        : short === 127
          ? Number(bytes.readBigUInt64BE(at + 2))
          : short;
    const mask = bytes.subarray(start, start + 4);
    const payload = Buffer.from(bytes.subarray(start + 4, start + 4 + length));
    for (let index = 0; index < payload.length; index++) {
      payload[index] = (payload[index] ?? 0) ^ (mask[index & 3] ?? 0);
    }
    frames.push({
      opcode: (bytes[at] ?? 0) & 0x0f,
      masked: ((bytes[at + 1] ?? 0) & 0x80) !== 0,
      payload,
    });
    at = start + 4 + length;
  }
  return frames;
}

/** A TCP server that answers the upgrade with `answer` and records what the client sends. */
function server(answer: (key: string) => string = upgraded) {
  const peer = { socket: null as Socket<{ head: string }> | null, received: Buffer.alloc(0) };
  const listener = Bun.listen<{ head: string }>({
    hostname: "127.0.0.1",
    port: 0,
    socket: {
      open: (socket) => {
        socket.data = { head: "" };
      },
      data: (socket, chunk) => {
        if (peer.socket !== null) {
          peer.received = Buffer.concat([peer.received, chunk]);
          return;
        }
        socket.data.head += chunk.toString("latin1");
        if (!socket.data.head.includes("\r\n\r\n")) return;
        expect(socket.data.head).toContain("authorization: Basic secret");
        peer.socket = socket;
        socket.write(answer(/sec-websocket-key: (\S+)/i.exec(socket.data.head)?.[1] ?? ""));
      },
    },
  });
  servers.push(listener);
  const open = () =>
    UpstreamSocket.connect(
      `ws://127.0.0.1:${listener.port}/api/pty/pty_1/connect`,
      { authorization: "Basic secret" },
      limit,
      2000,
    );
  return { peer, open };
}

function listen(socket: UpstreamSocket) {
  const seen = { text: "", closed: null as boolean | null };
  socket.start(
    (text) => {
      seen.text += text;
    },
    (failed) => {
      seen.closed = failed;
    },
  );
  return seen;
}

describe("upstream WebSocket", () => {
  test("opens only on the server's accept key", async () => {
    expect(await server().open()).toBeInstanceOf(UpstreamSocket);
    const wrong = server(
      () => "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: x\r\n\r\n",
    );
    await expect(wrong.open()).rejects.toThrow("the upgrade was refused");
    const refused = server(() => "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n");
    await expect(refused.open()).rejects.toThrow("the upgrade was refused");
  });

  test("passes text on across frames, skips binary messages and answers pings", async () => {
    const { peer, open } = server();
    const socket = await open();
    // Sent before start(): it waits in the connection.
    peer.socket?.write(
      Buffer.concat([
        frame(0x1, Buffer.from([0x61, 0xc3]), false),
        frame(0x9, "ping"),
        frame(0x0, Buffer.from([0xa9, 0x62])),
        frame(0x2, Buffer.from([0, 0x7b]), false),
        frame(0x0, "skipped"),
        frame(0x1, "c".repeat(limit - 1)),
      ]),
    );
    await Bun.sleep(50);
    const seen = listen(socket);
    await until(() => seen.text.length === limit + 2, "the text");
    expect(seen.text).toBe(`aéb${"c".repeat(limit - 1)}`);
    await until(() => peer.received.length > 0, "the pong");
    expect(clientFrames(peer.received)).toEqual([
      { opcode: 0xa, masked: true, payload: Buffer.from("ping") },
    ]);

    peer.socket?.write(frame(0x8, Buffer.from([0x03, 0xe8])));
    await until(() => seen.closed !== null, "the close");
    expect(seen.closed).toBe(false);
  });

  test("closes on a frame over its limit before reading the payload", async () => {
    const { peer, open } = server();
    const seen = listen(await open());
    // 2^40 bytes announced; none follow.
    peer.socket?.write(Buffer.from([0x81, 127, 0, 0, 1, 0, 0, 0, 0, 0]));
    await until(() => seen.closed !== null, "the close");
    expect(seen.closed).toBe(true);

    const masked = server();
    const second = listen(await masked.open());
    masked.peer.socket?.write(Buffer.from([0x81, 0x81, 1, 2, 3, 4, 0x61]));
    await until(() => second.closed !== null, "the close");
    expect(second.closed).toBe(true);
    expect(second.text).toBe("");
  });

  test("sends masked text frames of every length and a close frame", async () => {
    const { peer, open } = server();
    const socket = await open();
    listen(socket);
    const texts = ["short", "m".repeat(300), "l".repeat(70_000)];
    for (const text of texts) socket.send(text);
    socket.close();
    await until(() => clientFrames(peer.received).length === 4, "the frames");
    expect(clientFrames(peer.received)).toEqual([
      ...texts.map((text) => ({ opcode: 0x1, masked: true, payload: Buffer.from(text) })),
      { opcode: 0x8, masked: true, payload: Buffer.from([0x03, 0xe8]) },
    ]);
  });
});
