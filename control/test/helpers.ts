import { afterEach, expect } from "bun:test";
import type { Server, ServerWebSocket } from "bun";
import { randomBytes } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "../src/agent";
import { csrfHeader, type Role } from "../src/api";
import { Auth } from "../src/auth";
import { BrokerFiles } from "../src/broker";
import { Secrets, totpCode } from "../src/crypto";
import { openDatabase } from "../src/db";
import { type ContainerSpec, type Docker, DockerError } from "../src/docker";
import { Environments } from "../src/environments";
import { Keys } from "../src/keys";
import { sessionCookie, startServer } from "../src/server";
import { uiServer } from "../src/ui";

export const origin = "https://tentaflake.example";
export const setupToken = "setup-token-for-tests-0123456789";
export const password = "correct horse battery staple";
export const gateway = "192.0.2.1";
export const brokerUrl = `http://${gateway}:7810/v1`;

/** An in-memory Docker Engine; a method named in `failing` throws like a failed API call. */
export class FakeDocker implements Docker {
  readonly networks = new Map<string, boolean>();
  readonly volumes = new Set<string>();
  readonly containers = new Map<string, { name: string; spec: ContainerSpec; running: boolean }>();
  readonly failing = new Set<keyof Docker>();
  /** Runs when a container stops or is removed; Docker answers after it. */
  onStop: () => Promise<void> = async () => {};
  private nextId = 1;

  async network(name: string) {
    this.check("network");
    const internal = this.networks.get(name);
    return internal === undefined ? null : { internal, gateway };
  }
  async createNetwork(name: string, internal: boolean) {
    this.check("createNetwork");
    this.networks.set(name, internal);
  }
  async createVolume(name: string) {
    this.check("createVolume");
    this.volumes.add(name);
  }
  async removeVolume(name: string) {
    this.check("removeVolume");
    this.volumes.delete(name);
  }
  async createContainer(name: string, spec: ContainerSpec) {
    this.check("createContainer");
    const id = `container-${this.nextId++}`;
    this.containers.set(id, { name, spec, running: false });
    return id;
  }
  async startContainer(id: string) {
    this.check("startContainer");
    this.container(id).running = true;
  }
  async stopContainer(id: string) {
    this.check("stopContainer");
    await this.onStop();
    this.container(id).running = false;
  }
  async removeContainer(idOrName: string) {
    this.check("removeContainer");
    for (const [id, { name }] of this.containers) {
      if (id === idOrName || name === idOrName) this.containers.delete(id);
    }
    await this.onStop();
  }
  /** Running containers are at loopback, where the harness's FakeOpenCode listens. */
  async containerAddress(id: string) {
    this.check("containerAddress");
    return this.containers.get(id)?.running ? "127.0.0.1" : null;
  }

  private container(id: string) {
    const container = this.containers.get(id);
    if (container === undefined) throw new DockerError(404, `no such container: ${id}`);
    return container;
  }
  private check(method: keyof Docker) {
    if (this.failing.has(method)) throw new DockerError(500, `fake ${method} failure`);
  }
}

export interface OpenCodeRequest {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: unknown;
}

const encoder = new TextEncoder();

/**
 * A fake OpenCode server with the API the session proxy uses (observed on
 * OpenCode 2.0.29): it records every request, keeps sessions, permission
 * requests and files, relays `emit`ted text as its event stream and echoes
 * PTY input as output. With `failing`, every request answers 500.
 */
export class FakeOpenCode {
  readonly requests: OpenCodeRequest[] = [];
  sessions: Record<string, unknown>[] = [];
  /** Session id → { type: "running" } while a prompt runs. */
  active: Record<string, unknown> = {};
  permissions: Record<string, unknown>[] = [];
  files: Record<string, string> = {};
  failing = false;
  /** Runs before OpenCode answers a PTY creation. */
  beforePty: () => Promise<unknown> = async () => {};
  readonly pty = { input: [] as string[], resized: [] as unknown[], removed: [] as string[] };
  private readonly streams = new Set<ReadableStreamDefaultController<Uint8Array>>();
  private readonly sockets = new Set<ServerWebSocket<unknown>>();
  private readonly server: Server<unknown>;

  constructor() {
    this.server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: (request, server) => this.handle(request, server),
      websocket: {
        open: (socket) => {
          this.sockets.add(socket);
          socket.send(new Uint8Array([0, ...encoder.encode('{"cursor":0}')]));
          socket.send("$ ");
        },
        message: (_, data) => {
          this.pty.input.push(String(data));
        },
        close: (socket) => {
          this.sockets.delete(socket);
        },
      },
    });
  }

  get port(): number {
    return this.server.port ?? 0;
  }

  /** Sends raw server-sent-event text on every open event stream. */
  emit(text: string): void {
    for (const stream of this.streams) stream.enqueue(encoder.encode(text));
  }

  /** Sends terminal output on every PTY connection. */
  output(text: string): void {
    for (const socket of this.sockets) socket.send(text);
  }

  /** Ends every event stream and PTY connection, as OpenCode does when its container stops. */
  drop(): void {
    for (const stream of this.streams) stream.close();
    this.streams.clear();
    for (const socket of this.sockets) socket.close();
  }

  get openStreams(): number {
    return this.streams.size;
  }

  get openSockets(): number {
    return this.sockets.size;
  }

  stop(): void {
    this.server.stop(true);
  }

  private async handle(request: Request, server: Server<unknown>) {
    const url = new URL(request.url);
    const body = request.headers.has("content-type") ? await request.json() : undefined;
    const headers = Object.fromEntries(request.headers);
    this.requests.push({ method: request.method, path: url.pathname + url.search, headers, body });
    if (this.failing) return new Response("failure", { status: 500 });
    const route = `${request.method} ${url.pathname}`;
    const location = { directory: "/workspace" };
    const notFound = () => Response.json({ _tag: "NotFoundError" }, { status: 404 });
    const session = route.match(/^POST \/api\/session\/([^/]+)\/(prompt|interrupt)$/);
    const reply = route.match(/^POST \/api\/session\/([^/]+)\/permission\/([^/]+)\/reply$/);
    const read = route.match(/^GET \/api\/fs\/read\/(.+)$/);
    const pty = route.match(/^(PUT|DELETE) \/api\/pty\/([^/]+)$/);

    if (route === "GET /api/session") return Response.json({ data: this.sessions, cursor: {} });
    if (route === "POST /api/session") {
      const id = `ses_fake${this.sessions.length + 1}`;
      const session = { id, projectID: "p", time: { created: 1, updated: 2 }, location };
      this.sessions.push(session);
      return Response.json({ data: session });
    }
    if (route === "GET /api/session/active") return Response.json({ data: this.active });
    if (session) {
      const [, id = "", action] = session;
      if (!this.sessions.some((session) => session.id === id)) return notFound();
      if (action === "interrupt")
        return Response.json({ interrupted: Object.hasOwn(this.active, id) });
      return Response.json({ data: { id: "msg_fake1", sessionID: id, type: "user" } });
    }
    if (route === "GET /api/permission/request") {
      return Response.json({ location, data: this.permissions });
    }
    if (reply) {
      const index = this.permissions.findIndex(
        (permission) => permission.sessionID === reply[1] && permission.id === reply[2],
      );
      if (index === -1) return notFound();
      this.permissions.splice(index, 1);
      return new Response(null, { status: 204 });
    }
    if (route === "GET /api/fs/list") {
      const directory = url.searchParams.get("path") ?? "";
      const prefix = directory === "" ? "" : `${directory}/`;
      const entries = new Map<string, string>();
      for (const file of Object.keys(this.files)) {
        if (!file.startsWith(prefix)) continue;
        const [name = "", ...rest] = file.slice(prefix.length).split("/");
        entries.set(
          `${prefix}${name}${rest.length > 0 ? "/" : ""}`,
          rest.length > 0 ? "directory" : "file",
        );
      }
      if (entries.size === 0 && directory !== "") return notFound();
      const data = [...entries].map(([path, type]) => ({ path, type }));
      return Response.json({ location, data });
    }
    if (read) {
      const content = this.files[decodeURIComponent(read[1] ?? "")];
      return content === undefined ? notFound() : new Response(content);
    }
    if (route === "GET /api/event") {
      let self: ReadableStreamDefaultController<Uint8Array>;
      const stream = new ReadableStream<Uint8Array>({
        start: (controller) => {
          self = controller;
          this.streams.add(controller);
          controller.enqueue(encoder.encode('data: {"type":"server.connected","data":{}}\n\n'));
        },
        cancel: () => {
          this.streams.delete(self);
        },
      });
      server.timeout(request, 0);
      return new Response(stream, { headers: { "content-type": "text/event-stream" } });
    }
    if (route === "POST /api/pty") {
      await this.beforePty();
      const data = {
        id: "pty_fake1",
        title: "t",
        command: "/bin/bash",
        args: [],
        cwd: "/workspace",
      };
      return Response.json({ location, data: { ...data, status: "running", pid: 2 } });
    }
    if (pty) {
      if (pty[1] === "PUT") this.pty.resized.push(body);
      else this.pty.removed.push(pty[2] ?? "");
      return new Response(null, { status: 204 });
    }
    if (route === "GET /api/pty/pty_fake1/connect" && server.upgrade(request, { data: {} })) {
      return undefined;
    }
    return notFound();
  }
}

/** Waits up to about two seconds for `condition`. */
export async function until(condition: () => boolean, what: string): Promise<void> {
  for (let attempt = 0; !condition(); attempt++) {
    if (attempt > 200) throw new Error(`timed out waiting for ${what}`);
    await Bun.sleep(10);
  }
}

export type Harness = ReturnType<typeof harness>;
const running: { close(): void }[] = [];
afterEach(() => {
  for (const h of running.splice(0)) h.close();
});

/**
 * The service on a random loopback port with an in-memory database and a fake Docker.
 * With `ui` it serves that built UI; the browser journeys also pick the port and origin.
 */
export function harness({
  token = setupToken as string | null,
  ui = undefined as string | undefined,
  port = 0,
  origin: listenOrigin = origin as string,
} = {}) {
  const dir = mkdtempSync(join(tmpdir(), "tentaflake-control-"));
  const setupTokenFile = join(dir, "setup-token");
  if (token !== null) writeFileSync(setupTokenFile, `${token}\n`);
  const clock = { now: Date.UTC(2026, 0, 1) };
  const now = () => clock.now;
  const db = openDatabase(":memory:");
  const secrets = new Secrets(randomBytes(32));
  const docker = new FakeDocker();
  const sessions = { setupTokenFile, sessionIdleMinutes: 60, sessionAbsoluteHours: 12 };
  const auth = new Auth(db, secrets, sessions, now);
  const limits = { memoryMiB: 1024, cpus: 1.5, pids: 256, dailyTokens: 50_000 };
  const brokerDirectory = join(dir, "broker");
  const broker = new BrokerFiles(db, secrets, brokerDirectory);
  const opencode = new FakeOpenCode();
  // Docker answers a stop only after the processes, OpenCode among them, ended.
  docker.onStop = async () => {
    opencode.drop();
    await Bun.sleep(50);
  };
  const config = { image: "runtime:test", limits, brokerUrl, opencodePort: opencode.port };
  const environments = new Environments(db, docker, secrets, broker, config, now);
  const keys = new Keys(db, secrets, broker, now);
  const agent = new Agent(environments);
  const listen = { hostname: "127.0.0.1", port, origin: listenOrigin };
  const server = startServer(
    listen,
    auth,
    environments,
    keys,
    agent,
    ui === undefined ? undefined : uiServer(ui, listenOrigin),
  );
  const h = {
    url: server.url.origin,
    clock,
    now,
    db,
    secrets,
    docker,
    opencode,
    auth,
    environments,
    keys,
    agent,
    broker,
    brokerDirectory,
    close() {
      server.stop(true);
      opencode.stop();
      db.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
  running.push(h);
  return h;
}

export interface Client {
  cookie?: string;
  csrf?: string;
  /** The user's model key (newUser adds one). */
  keyId?: string;
}

/** A JSON API request; mutations carry the configured Origin unless `origin` overrides it. */
export function call(
  h: Harness,
  method: string,
  path: string,
  body?: unknown,
  client: Client = {},
  { origin: requestOrigin = origin as string | null } = {},
): Promise<Response> {
  const headers: Record<string, string> = {};
  if (method !== "GET" && requestOrigin !== null) headers.origin = requestOrigin;
  if (body !== undefined) headers["content-type"] = "application/json";
  if (client.cookie !== undefined) headers.cookie = `${sessionCookie}=${client.cookie}`;
  if (client.csrf !== undefined) headers[csrfHeader] = client.csrf;
  return fetch(`${h.url}${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

/** The current TOTP code; advances the clock one step first so each code is fresh. */
export function nextCode(h: Harness, secret: Buffer): string {
  h.clock.now += 30_000;
  return totpCode(secret, Math.floor(h.clock.now / 30_000));
}

export async function signIn(h: Harness, username: string, secret: Buffer): Promise<Client> {
  const code = nextCode(h, secret);
  const response = await call(h, "POST", "/api/v1/session", { username, password, code });
  expect(response.status).toBe(200);
  const cookie = /__Host-tentaflake=([^;]+)/.exec(response.headers.get("set-cookie") ?? "")?.[1];
  const { csrfToken } = (await response.json()) as { csrfToken: string };
  return { cookie, csrf: csrfToken };
}

/** The secret of the anthropic key newUser adds for a user. */
export const providerSecret = (username: string) => `sk-ant-fixture-${username}-0123456789`;

/** Creates a user directly with one anthropic key and signs it in. */
export async function newUser(h: Harness, username: string, role: Role = "member") {
  const { user, totpSecret } = await h.auth.createUser(username, password, role);
  const key = h.keys.add(user.id, {
    provider: "anthropic",
    label: "default",
    secret: providerSecret(username),
  });
  const client = { ...(await signIn(h, username, totpSecret)), keyId: key.id };
  return { user, totpSecret, client };
}

/** Creates an environment ("dev", the user's key, one model) with `body` overriding. */
export const createEnvironment = (h: Harness, client: Client, body: object = {}) =>
  call(
    h,
    "POST",
    "/api/v1/environments",
    { name: "dev", keyId: client.keyId, models: ["fixture-model"], ...body },
    client,
  );

/** The environment variables of a container spec as an object. */
export const envOf = (env: string[]) =>
  Object.fromEntries(
    env.map((entry) => [entry.split("=", 1)[0], entry.slice(entry.indexOf("=") + 1)]),
  );

export function base32Decode(text: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  const bytes: number[] = [];
  let value = 0;
  let bits = 0;
  for (const char of text) {
    value = ((value << 5) | alphabet.indexOf(char)) & 0x1fff;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((value >>> bits) & 0xff);
    }
  }
  return Buffer.from(bytes);
}
