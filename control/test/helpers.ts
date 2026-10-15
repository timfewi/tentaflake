import { afterEach, expect } from "bun:test";
import { randomBytes } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { csrfHeader, type Role } from "../src/api";
import { Auth } from "../src/auth";
import { BrokerFiles } from "../src/broker";
import { Secrets, totpCode } from "../src/crypto";
import { openDatabase } from "../src/db";
import { type ContainerSpec, type Docker, DockerError } from "../src/docker";
import { Environments } from "../src/environments";
import { Keys } from "../src/keys";
import { sessionCookie, startServer } from "../src/server";

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
    this.container(id).running = false;
  }
  async removeContainer(idOrName: string) {
    this.check("removeContainer");
    for (const [id, { name }] of this.containers) {
      if (id === idOrName || name === idOrName) this.containers.delete(id);
    }
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

export type Harness = ReturnType<typeof harness>;
const running: { close(): void }[] = [];
afterEach(() => {
  for (const h of running.splice(0)) h.close();
});

/** The service on a random loopback port with an in-memory database and a fake Docker. */
export function harness({ token = setupToken as string | null } = {}) {
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
  const config = { image: "runtime:test", limits, brokerUrl };
  const environments = new Environments(db, docker, secrets, broker, config, now);
  const keys = new Keys(db, secrets, broker, now);
  const server = startServer({ hostname: "127.0.0.1", port: 0, origin }, auth, environments, keys);
  const h = {
    url: server.url.origin,
    clock,
    db,
    secrets,
    docker,
    auth,
    environments,
    keys,
    broker,
    brokerDirectory,
    close() {
      server.stop(true);
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
