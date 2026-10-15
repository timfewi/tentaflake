import { describe, expect, test } from "bun:test";
import { fileLimit, IdleStop, terminalFrameLimit, upstreamFrameLimit } from "../src/agent";
import type { AgentEvent, AgentSession, Environment, PermissionList } from "../src/api";
import { sessionCookie } from "../src/server";
import {
  type Client,
  call,
  createEnvironment,
  type Harness,
  harness,
  newUser,
  origin,
  until,
} from "./helpers";

async function running(h: Harness, client: Client): Promise<Environment> {
  const response = await createEnvironment(h, client);
  expect(response.status).toBe(201);
  return (await response.json()) as Environment;
}

/** A signed-in owner with a running environment, and its API path prefix. */
async function setup(h = harness()) {
  const { user, client } = await newUser(h, "owner");
  const env = await running(h, client);
  return { h, user, client, env, path: `/api/v1/environments/${env.id}` };
}

const activity = (h: Harness, id: string) =>
  (
    h.db.query("SELECT last_activity_at FROM environments WHERE id = ?").get(id) as {
      last_activity_at: number;
    }
  ).last_activity_at;

/** Reads server-sent events from the service until `count` arrived or the stream ended. */
async function readEvents(response: Response, count: number) {
  const events: AgentEvent[] = [];
  const reader = (response.body as ReadableStream<Uint8Array>).getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let ended = false;
  while (events.length < count) {
    const { done, value } = await reader.read();
    if (done) {
      ended = true;
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    for (let index = buffer.indexOf("\n\n"); index !== -1; index = buffer.indexOf("\n\n")) {
      const frame = buffer.slice(0, index);
      buffer = buffer.slice(index + 2);
      if (frame.startsWith("data: ")) events.push(JSON.parse(frame.slice(6)));
    }
  }
  return { events, ended, reader };
}

/** Opens the terminal WebSocket; resolves once it is open or closed. */
function terminal(h: Harness, client: Client, path: string, headers: Record<string, string> = {}) {
  const socket = new WebSocket(`${h.url.replace("http", "ws")}${path}/terminal`, {
    headers: { cookie: `${sessionCookie}=${client.cookie}`, origin, ...headers },
  });
  const received: string[] = [];
  const closed = new Promise<{ code: number; reason: string }>((resolve) => {
    socket.onclose = ({ code, reason }) => resolve({ code, reason });
  });
  socket.onmessage = ({ data }) => received.push(String(data));
  const opened = new Promise<boolean>((resolve) => {
    socket.onopen = () => resolve(true);
    closed.then(() => resolve(false));
  });
  return { socket, received, closed, opened };
}

describe("session proxy", () => {
  test("talks to OpenCode with the environment's password, never with the browser's headers", async () => {
    const { h, user, client, env, path } = await setup();
    h.opencode.active = { ses_fake1: { type: "running" } };

    const created = await call(h, "POST", `${path}/sessions`, undefined, client);
    expect(created.status).toBe(201);
    const session = (await created.json()) as AgentSession;
    expect(session).toEqual({
      id: "ses_fake1",
      title: null,
      createdAt: new Date(1).toISOString(),
      updatedAt: new Date(2).toISOString(),
      running: false,
    });
    const listed = await call(h, "GET", `${path}/sessions`, undefined, client);
    expect(await listed.json()).toEqual({ sessions: [{ ...session, running: true }] });

    const prompt = await call(
      h,
      "POST",
      `${path}/sessions/${session.id}/prompt`,
      { text: "write hello.py" },
      client,
    );
    expect(prompt.status).toBe(202);
    expect(await prompt.json()).toEqual({ messageId: "msg_fake1" });
    const abort = await call(h, "POST", `${path}/sessions/${session.id}/abort`, undefined, client);
    expect(await abort.json()).toEqual({ aborted: true });

    expect(h.opencode.requests.map(({ method, path }) => `${method} ${path}`)).toEqual([
      "POST /api/session",
      "GET /api/session?parentID=null",
      "GET /api/session/active",
      "POST /api/session/ses_fake1/prompt",
      "POST /api/session/ses_fake1/interrupt",
    ]);
    expect(h.opencode.requests[3]?.body).toEqual({ text: "write hello.py" });
    const password = h.environments.opencodePassword(user.id, env.id);
    for (const { headers } of h.opencode.requests) {
      expect(headers.authorization).toBe(`Basic ${btoa(`opencode:${password}`)}`);
      for (const name of ["cookie", "origin", "x-csrf-token"])
        expect(headers[name]).toBeUndefined();
    }
  });

  test("of another user's environment answers 404 and never reaches OpenCode", async () => {
    const { h, path } = await setup();
    const other = await newUser(h, "other", "admin");
    for (const [method, suffix, body] of [
      ["GET", "/sessions"],
      ["POST", "/sessions"],
      ["POST", "/sessions/ses_fake1/prompt", { text: "hi" }],
      ["POST", "/sessions/ses_fake1/abort"],
      ["GET", "/permissions"],
      ["POST", "/sessions/ses_fake1/permissions/per_fake1", { reply: "once" }],
      ["GET", "/files"],
      ["GET", "/files/content?path=a"],
      ["GET", "/events"],
    ] as const) {
      const response = await call(h, method, `${path}${suffix}`, body, other.client);
      expect(response.status).toBe(404);
    }
    const shell = terminal(h, other.client, path);
    expect(await shell.opened).toBe(false);
    expect(h.opencode.requests).toEqual([]);
  });

  test("needs a running environment and reports OpenCode failures", async () => {
    const { h, client, path } = await setup();
    expect(
      (await call(h, "POST", `${path}/sessions/ses_none/abort`, undefined, client)).status,
    ).toBe(404);
    expect((await call(h, "POST", `${path}/sessions/bad/abort`, undefined, client)).status).toBe(
      404,
    );
    for (const text of ["", " ", 5, "x".repeat(32_769)]) {
      const prompt = await call(h, "POST", `${path}/sessions/ses_x/prompt`, { text }, client);
      expect(prompt.status).toBe(400);
    }

    h.opencode.sessions = [{ id: "../escape", time: { created: 1, updated: 1 } }];
    const invalid = await call(h, "GET", `${path}/sessions`, undefined, client);
    expect(invalid.status).toBe(502);
    expect(await invalid.json()).toMatchObject({ error: "opencode" });

    h.opencode.failing = true;
    expect((await call(h, "GET", `${path}/sessions`, undefined, client)).status).toBe(502);
    h.opencode.failing = false;

    await call(h, "POST", `${path}/stop`, undefined, client);
    const stopped = await call(h, "GET", `${path}/sessions`, undefined, client);
    expect(stopped.status).toBe(409);
    expect((await call(h, "GET", `${path}/events`, undefined, client)).status).toBe(409);
  });
});

describe("permissions", () => {
  test("are listed, and each reply reaches OpenCode as its decision", async () => {
    const { h, client, path } = await setup();
    const request = (id: string) => ({
      id,
      sessionID: "ses_fake1",
      action: "shell",
      resources: ["echo hi > hello.txt"],
      save: ["echo *"],
      source: { type: "tool", messageID: "msg_1", id: "call_1" },
    });
    h.opencode.permissions = ["per_a", "per_b", "per_c"].map(request);
    const listed = await call(h, "GET", `${path}/permissions`, undefined, client);
    expect(((await listed.json()) as PermissionList).permissions[0]).toEqual({
      id: "per_a",
      sessionId: "ses_fake1",
      action: "shell",
      resources: ["echo hi > hello.txt"],
    });

    for (const [id, reply] of [
      ["per_a", "once"],
      ["per_b", "always"],
      ["per_c", "reject"],
    ]) {
      const replied = await call(
        h,
        "POST",
        `${path}/sessions/ses_fake1/permissions/${id}`,
        { reply },
        client,
      );
      expect(replied.status).toBe(204);
      expect(h.opencode.requests.at(-1)).toMatchObject({
        method: "POST",
        path: `/api/session/ses_fake1/permission/${id}/reply`,
        body: { decision: reply },
      });
    }
    expect(h.opencode.permissions).toEqual([]);

    const sent = h.opencode.requests.length;
    for (const reply of ["allow", "ONCE", "", 1, undefined]) {
      const response = await call(
        h,
        "POST",
        `${path}/sessions/ses_fake1/permissions/per_a`,
        { reply },
        client,
      );
      expect(response.status).toBe(400);
    }
    expect(h.opencode.requests.length).toBe(sent);
    const gone = await call(
      h,
      "POST",
      `${path}/sessions/ses_fake1/permissions/per_a`,
      { reply: "once" },
      client,
    );
    expect(gone.status).toBe(404);
  });
});

describe("files", () => {
  test("are listed and read inside /workspace, as bytes a browser does not render", async () => {
    const { h, client, path } = await setup();
    h.opencode.files = { "hello.py": "print('hello')\n", "src/main.ts": "<script>x</script>" };
    const root = await call(h, "GET", `${path}/files`, undefined, client);
    expect(await root.json()).toEqual({
      entries: [
        { path: "hello.py", type: "file" },
        { path: "src", type: "directory" },
      ],
    });
    const src = await call(h, "GET", `${path}/files?path=src`, undefined, client);
    expect(await src.json()).toEqual({ entries: [{ path: "src/main.ts", type: "file" }] });

    const read = await call(h, "GET", `${path}/files/content?path=src/main.ts`, undefined, client);
    expect(read.status).toBe(200);
    expect(await read.text()).toBe("<script>x</script>");
    expect(read.headers.get("content-type")).toBe("application/octet-stream");
    expect(read.headers.get("content-disposition")).toBe("attachment");
    expect(read.headers.get("x-content-type-options")).toBe("nosniff");
    expect(read.headers.get("content-security-policy")).toBe("sandbox");
    expect(h.opencode.requests.at(-1)?.path).toBe("/api/fs/read/src/main.ts");

    const missing = await call(h, "GET", `${path}/files/content?path=nope`, undefined, client);
    expect(missing.status).toBe(404);
  });

  test("outside the workspace are refused before OpenCode is asked", async () => {
    const { h, client, path } = await setup();
    for (const target of [
      "..",
      "../etc/passwd",
      "src/../../etc/passwd",
      "/etc/passwd",
      "./hello.py",
      "src//main.ts",
      "src/",
      "a\u0000b",
      "x".repeat(4097),
    ]) {
      const query = new URLSearchParams({ path: target });
      for (const suffix of ["/files", "/files/content"]) {
        const response = await call(h, "GET", `${path}${suffix}?${query}`, undefined, client);
        expect(response.status).toBe(400);
      }
    }
    const empty = await call(h, "GET", `${path}/files/content`, undefined, client);
    expect(empty.status).toBe(400);
    expect(h.opencode.requests).toEqual([]);
  });

  test("over the size limit are refused", async () => {
    const { h, client, path } = await setup();
    h.opencode.files = { exact: "x".repeat(fileLimit), large: "x".repeat(fileLimit + 1) };
    const exact = await call(h, "GET", `${path}/files/content?path=exact`, undefined, client);
    expect((await exact.arrayBuffer()).byteLength).toBe(fileLimit);
    const large = await call(h, "GET", `${path}/files/content?path=large`, undefined, client);
    expect(large.status).toBe(413);
    expect(await large.json()).toMatchObject({ error: "too_large" });
  });
});

describe("events", () => {
  test("relay only the UI's events, validated and reduced to their fields", async () => {
    const { h, client, path } = await setup();
    const response = await call(h, "GET", `${path}/events`, undefined, client);
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("text/event-stream");
    await until(() => h.opencode.openStreams === 1, "the upstream stream");

    const location = { directory: "/workspace" };
    const data = (type: string, fields: object) =>
      `data: ${JSON.stringify({ id: "evt_1", type, location, data: fields, durable: { seq: 1 } })}\n\n`;
    const session = { sessionID: "ses_fake1" };
    const message = { ...session, assistantMessageID: "msg_2" };
    h.opencode.emit(
      [
        ": heartbeat\n\n",
        data("project.updated", { id: "p", canonical: "/workspace" }),
        data("session.instructions.updated", { ...session, delta: { "core/date": "abc" } }),
        "data: {not json\n\n",
        data("session.created", { sessionID: 7 }),
        data("constructor", session),
        data("session.created", { ...session, slug: "calm-wolf", location }),
        data("session.execution.started", session),
        data("session.text.delta", { ...message, ordinal: 0, delta: "Hel" }),
        data("session.text.ended", { ...message, ordinal: 0, text: "Hello" }),
        data("session.tool.input.started", { ...message, id: "call_1", name: "shell" }),
        data("session.tool.called", { ...message, id: "call_1", input: { command: "ls" } }),
        data("permission.asked", {
          id: "per_1",
          ...session,
          action: "shell",
          resources: ["ls"],
          save: ["ls *"],
        }),
        data("permission.replied", { ...session, requestID: "per_1", reply: "once" }),
        data("session.tool.success", {
          ...message,
          id: "call_1",
          content: [{ type: "text", text: "hello.py\n" }],
          metadata: { exit: 0 },
        }),
        data("session.tool.failed", {
          ...message,
          id: "call_2",
          error: { type: "x", message: "no" },
        }),
        data("session.execution.failed", { ...session, error: { type: "api", message: "402" } }),
        data("session.renamed", { ...session, title: "Hello" }),
      ].join(""),
    );
    const { events } = await readEvents(response, 12);
    expect(events).toEqual([
      { type: "session.created", sessionId: "ses_fake1" },
      { type: "session.execution.started", sessionId: "ses_fake1" },
      { type: "session.text.delta", sessionId: "ses_fake1", messageId: "msg_2", delta: "Hel" },
      { type: "session.text.ended", sessionId: "ses_fake1", messageId: "msg_2", text: "Hello" },
      {
        type: "session.tool.input.started",
        sessionId: "ses_fake1",
        messageId: "msg_2",
        callId: "call_1",
        name: "shell",
      },
      {
        type: "session.tool.called",
        sessionId: "ses_fake1",
        messageId: "msg_2",
        callId: "call_1",
        input: { command: "ls" },
      },
      {
        type: "permission.asked",
        id: "per_1",
        sessionId: "ses_fake1",
        action: "shell",
        resources: ["ls"],
      },
      { type: "permission.replied", sessionId: "ses_fake1", permissionId: "per_1", reply: "once" },
      {
        type: "session.tool.success",
        sessionId: "ses_fake1",
        messageId: "msg_2",
        callId: "call_1",
        output: "hello.py\n",
      },
      {
        type: "session.tool.failed",
        sessionId: "ses_fake1",
        messageId: "msg_2",
        callId: "call_2",
        error: "no",
      },
      { type: "session.execution.failed", sessionId: "ses_fake1", error: "402" },
      { type: "session.renamed", sessionId: "ses_fake1", title: "Hello" },
    ]);
  });

  test("drop an event over the frame limit and continue", async () => {
    const { h, client, path } = await setup();
    const response = await call(h, "GET", `${path}/events`, undefined, client);
    await until(() => h.opencode.openStreams === 1, "the upstream stream");
    const title = "x".repeat(1_100_000);
    const huge = { type: "session.renamed", data: { sessionID: "ses_fake1", title } };
    h.opencode.emit(`data: ${JSON.stringify(huge)}\n\n`);
    h.opencode.emit('data: {"type":"session.deleted","data":{"sessionID":"ses_fake1"}}\n\n');
    const { events } = await readEvents(response, 1);
    expect(events).toEqual([{ type: "session.deleted", sessionId: "ses_fake1" }]);
  });

  test("end cleanly when the environment stops or is deleted", async () => {
    for (const [method, route, status] of [
      ["POST", "/stop", 200],
      ["DELETE", "", 204],
    ] as const) {
      const { h, client, path } = await setup();
      const response = await call(h, "GET", `${path}/events`, undefined, client);
      await until(() => h.opencode.openStreams === 1, "the upstream stream");
      expect((await call(h, method, `${path}${route}`, undefined, client)).status).toBe(status);
      const { events, ended } = await readEvents(response, 2);
      expect(events).toEqual([{ type: "environment.stopped" }]);
      expect(ended).toBe(true);
      await until(() => h.opencode.openStreams === 0, "the upstream stream to close");
    }
  });
});

describe("terminal", () => {
  test("relays input and output and resizes the PTY", async () => {
    const { h, client, path } = await setup();
    const shell = terminal(h, client, path);
    expect(await shell.opened).toBe(true);
    await until(() => shell.received.includes("$ "), "the prompt");

    shell.socket.send(JSON.stringify({ type: "input", data: "id -u\r" }));
    shell.socket.send(JSON.stringify({ type: "resize", rows: 40, cols: 120 }));
    await until(() => h.opencode.pty.input.length === 1, "the input");
    expect(h.opencode.pty.input).toEqual(["id -u\r"]);
    await until(() => h.opencode.pty.resized.length === 1, "the resize");
    expect(h.opencode.pty.resized).toEqual([{ size: { rows: 40, cols: 120 } }]);
    expect(h.opencode.requests[0]).toMatchObject({ method: "POST", path: "/api/pty" });

    // Output larger than a frame arrives in frames within the limit, surrogate pairs intact.
    const output = `${"a".repeat(terminalFrameLimit / 4 - 1)}😀${"b".repeat(terminalFrameLimit)}`;
    h.opencode.output(output);
    await until(() => shell.received.join("").length >= output.length + 2, "the output");
    expect(shell.received.join("")).toBe(`$ ${output}`);
    for (const frame of shell.received) {
      expect(Buffer.byteLength(frame)).toBeLessThanOrEqual(terminalFrameLimit);
      expect(frame.isWellFormed()).toBe(true);
    }

    shell.socket.close();
    await until(() => h.opencode.pty.removed.length === 1, "the PTY removal");
    expect(h.opencode.pty.removed).toEqual(["pty_fake1"]);
    await until(() => h.opencode.openSockets === 0, "the upstream socket to close");
  });

  test("closes on a frame over the limit or an invalid message", async () => {
    const { h, client, path } = await setup();
    const large = terminal(h, client, path);
    expect(await large.opened).toBe(true);
    large.socket.send(JSON.stringify({ type: "input", data: "x".repeat(terminalFrameLimit) }));
    // Bun drops the connection without a close frame.
    expect([1006, 1009]).toContain((await large.closed).code);

    for (const frame of ["ls\r", '{"type":"resize","rows":0,"cols":80}', new Uint8Array([1])]) {
      const shell = terminal(h, client, path);
      expect(await shell.opened).toBe(true);
      shell.socket.send(frame);
      expect((await shell.closed).code).toBe(1008);
    }
    expect(h.opencode.pty.input).toEqual([]);
    await until(() => h.opencode.pty.removed.length === 4, "every PTY removal");
  });

  test("closes when OpenCode sends a frame over its limit", async () => {
    const { h, client, path } = await setup();
    const shell = terminal(h, client, path);
    expect(await shell.opened).toBe(true);
    h.opencode.output("y".repeat(upstreamFrameLimit));
    await until(() => shell.received.join("").length === upstreamFrameLimit + 2, "the output");
    h.opencode.output("z".repeat(upstreamFrameLimit + 1));
    expect(await shell.closed).toEqual({
      code: 1011,
      reason: "OpenCode sent an invalid terminal frame",
    });
    expect(shell.received.join("")).not.toContain("z");
    await until(() => h.opencode.pty.removed.length === 1, "the PTY removal");
  });

  test("needs the configured origin and a running environment", async () => {
    const { h, client, path } = await setup();
    const foreign = terminal(h, client, path, { origin: "https://evil.example" });
    expect(await foreign.opened).toBe(false);
    const plain = await fetch(`${h.url}${path}/terminal`, {
      headers: { cookie: `${sessionCookie}=${client.cookie}`, origin },
    });
    expect(plain.status).toBe(400);
    expect(h.opencode.requests).toEqual([]);

    await call(h, "POST", `${path}/stop`, undefined, client);
    expect(await terminal(h, client, path).opened).toBe(false);
  });

  test("closes and removes the PTY when the environment stops", async () => {
    const { h, client, path } = await setup();
    const shell = terminal(h, client, path);
    expect(await shell.opened).toBe(true);
    expect((await call(h, "POST", `${path}/stop`, undefined, client)).status).toBe(200);
    expect(await shell.closed).toEqual({ code: 1000, reason: "the environment stopped" });
    await until(() => h.opencode.pty.removed.length === 1, "the PTY removal");
  });

  test("closes one that opens while the environment stops", async () => {
    const { h, client, path } = await setup();
    h.opencode.beforePty = () => call(h, "POST", `${path}/stop`, undefined, client);
    const shell = terminal(h, client, path);
    expect(await shell.opened).toBe(true);
    expect(await shell.closed).toEqual({ code: 1000, reason: "the environment stopped" });
    await until(() => h.opencode.pty.removed.length === 1, "the PTY removal");
    await until(() => h.opencode.openSockets === 0, "the upstream socket to close");
  });

  test("ends at once, like an event stream, while the environment stops", async () => {
    const { h, client, path } = await setup();
    let stopping = false;
    let stopped = () => {};
    h.docker.onStop = () => {
      stopping = true;
      return new Promise((resolve) => {
        stopped = resolve;
      });
    };
    const stop = call(h, "POST", `${path}/stop`, undefined, client);
    await until(() => stopping, "the stop");

    const response = await call(h, "GET", `${path}/events`, undefined, client);
    expect(response.status).toBe(200);
    expect(await readEvents(response, 2)).toMatchObject({
      events: [{ type: "environment.stopped" }],
      ended: true,
    });
    const shell = terminal(h, client, path);
    expect(await shell.closed).toEqual({ code: 1000, reason: "the environment stopped" });
    stopped();
    expect((await stop).status).toBe(200);
  });
});

describe("idle stop", () => {
  const minutes = (n: number) => n * 60_000;
  const state = (h: Harness, user: string, id: string) => h.environments.get(user, id).state;

  test("stops an environment idle for longer than the timeout; starting it again works", async () => {
    const { h, user, client, env, path } = await setup();
    const idle = new IdleStop(h.environments, h.agent, minutes(15), h.now);
    h.clock.now += minutes(14);
    await idle.run();
    expect(state(h, user.id, env.id)).toBe("running");

    h.clock.now += minutes(2);
    await idle.run();
    expect(state(h, user.id, env.id)).toBe("stopped");
    expect([...h.docker.containers.values()][0]?.running).toBe(false);
    expect(h.docker.volumes.size).toBe(2);

    expect((await call(h, "POST", `${path}/start`, undefined, client)).status).toBe(200);
    expect(state(h, user.id, env.id)).toBe("running");
  });

  test("keeps one with an open terminal or a running prompt", async () => {
    const { h, user, client, env, path } = await setup();
    const idle = new IdleStop(h.environments, h.agent, minutes(15), h.now);
    const shell = terminal(h, client, path);
    expect(await shell.opened).toBe(true);
    h.clock.now += minutes(60);
    await idle.run();
    expect(state(h, user.id, env.id)).toBe("running");

    shell.socket.close();
    await shell.closed;
    await until(() => !h.environments.terminalOpen(env.id), "the terminal to close");
    h.opencode.active = { ses_fake1: { type: "running" } };
    await idle.run();
    expect(state(h, user.id, env.id)).toBe("running");
    // The running prompt counted as activity.
    expect(activity(h, env.id)).toBe(h.clock.now);

    h.opencode.active = {};
    h.clock.now += minutes(16);
    await idle.run();
    expect(state(h, user.id, env.id)).toBe("stopped");
  });

  test("keeps one whose OpenCode does not answer; stops one whose container exited", async () => {
    const { h, user, env } = await setup();
    const idle = new IdleStop(h.environments, h.agent, minutes(15), h.now);
    h.clock.now += minutes(16);
    h.opencode.failing = true;
    await idle.run();
    expect(state(h, user.id, env.id)).toBe("running");

    const [container] = [...h.docker.containers.values()];
    if (container !== undefined) container.running = false;
    await idle.run();
    expect(state(h, user.id, env.id)).toBe("stopped");
  });

  test("counts prompts, approvals, file reads, events and terminal input as activity", async () => {
    const { h, client, env, path } = await setup();
    h.opencode.sessions = [{ id: "ses_fake1", time: { created: 1, updated: 1 } }];
    h.opencode.files = { "a.txt": "a" };
    const later = () => {
      h.clock.now += 60_000;
      return h.clock.now;
    };
    const actions: [string, () => Promise<unknown>][] = [
      [
        "prompt",
        () => call(h, "POST", `${path}/sessions/ses_fake1/prompt`, { text: "hi" }, client),
      ],
      [
        "approval",
        () => {
          h.opencode.permissions = [
            { id: "per_1", sessionID: "ses_fake1", action: "a", resources: [] },
          ];
          return call(
            h,
            "POST",
            `${path}/sessions/ses_fake1/permissions/per_1`,
            { reply: "once" },
            client,
          );
        },
      ],
      ["file read", () => call(h, "GET", `${path}/files/content?path=a.txt`, undefined, client)],
      ["listing", () => call(h, "GET", `${path}/files`, undefined, client)],
    ];
    for (const [name, action] of actions) {
      const time = later();
      const response = (await action()) as Response;
      expect([name, response.ok, activity(h, env.id)]).toEqual([name, true, time]);
    }

    // Listing sessions or permissions is no activity.
    const before = activity(h, env.id);
    later();
    await call(h, "GET", `${path}/sessions`, undefined, client);
    await call(h, "GET", `${path}/permissions`, undefined, client);
    expect(activity(h, env.id)).toBe(before);

    const events = await call(h, "GET", `${path}/events`, undefined, client);
    await until(() => h.opencode.openStreams === 1, "the upstream stream");
    let time = later();
    h.opencode.emit(
      'data: {"type":"session.execution.started","data":{"sessionID":"ses_fake1"}}\n\n',
    );
    await readEvents(events, 1);
    expect(activity(h, env.id)).toBe(time);

    const shell = terminal(h, client, path);
    expect(await shell.opened).toBe(true);
    time = later();
    shell.socket.send(JSON.stringify({ type: "input", data: "ls\r" }));
    await until(() => h.opencode.pty.input.length === 1, "the input");
    expect(activity(h, env.id)).toBe(time);
    shell.socket.close();
    await shell.closed;
  });
});
