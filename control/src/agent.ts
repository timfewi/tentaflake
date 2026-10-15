// The control service's side of OpenCode in an environment: sessions,
// prompts, events, approvals, files and the terminal, proxied for the
// environment's owner, and the idle stop. The browser never talks to OpenCode.
// Everything OpenCode answers comes from inside the environment, where the
// agent runs as the same user as OpenCode, so it is validated like input.

import type {
  AbortResponse,
  AgentEvent,
  AgentSession,
  FileEntry,
  Permission,
  PermissionReply,
  PromptResponse,
  TerminalMessage,
} from "./api";
import type { SessionAccess } from "./auth";
import { isObject } from "./config";
import type { Environments } from "./environments";
import { HttpError } from "./http";
import { UpstreamSocket } from "./websocket";

/** OpenCode in an environment did not answer, or not as expected. */
export class OpenCodeError extends Error {}

/** The largest file the API returns, in bytes. */
export const fileLimit = 1024 * 1024;
/** The largest terminal frame in either direction, in bytes. */
export const terminalFrameLimit = 64 * 1024;
// Output frames are cut by characters; at most three UTF-8 bytes each.
const outputFrameChars = terminalFrameLimit / 4;
/** The largest terminal frame from OpenCode, which replays output in frames of 64Ki characters. */
export const upstreamFrameLimit = 4 * terminalFrameLimit;
// An upstream event larger than this is dropped.
const eventFrameLimit = 1024 * 1024;
const answerLimit = 4 * 1024 * 1024;
const timeoutMs = 10_000;
const promptLimit = 32_768;

const ids = {
  session: /^ses_[A-Za-z0-9]{1,64}$/,
  permission: /^per_[A-Za-z0-9]{1,64}$/,
  pty: /^pty_[A-Za-z0-9]{1,64}$/,
};
const replies: readonly PermissionReply[] = ["once", "always", "reject"];

const unexpected = () => new OpenCodeError("OpenCode sent an unexpected answer");
const requireAccess = (access: SessionAccess) => {
  if (!access.check()) throw new HttpError(401, "unauthenticated", "sign in first");
};
const object = (value: unknown): Record<string, unknown> => {
  if (!isObject(value)) throw unexpected();
  return value;
};
const array = (value: unknown): unknown[] => {
  if (!Array.isArray(value)) throw unexpected();
  return value;
};
const string = (value: unknown, pattern?: RegExp): string => {
  if (typeof value !== "string" || (pattern !== undefined && !pattern.test(value))) {
    throw unexpected();
  }
  return value;
};
const date = (value: unknown): string => {
  const time = new Date(typeof value === "number" ? value : Number.NaN);
  if (Number.isNaN(time.getTime())) throw unexpected();
  return time.toISOString();
};

/** The body if it has at most `limit` bytes, else null. */
async function readAtMost(
  body: ReadableStream<Uint8Array> | null,
  limit: number,
): Promise<Uint8Array | null> {
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for await (const chunk of body ?? new ReadableStream<Uint8Array>()) {
      size += chunk.byteLength;
      // Leaving the loop cancels the stream.
      if (size > limit) return null;
      chunks.push(chunk);
    }
  } catch (error) {
    throw new OpenCodeError(`OpenCode's answer broke off: ${(error as Error).message}`);
  }
  return Buffer.concat(chunks);
}

interface RequestOptions {
  body?: unknown;
  signal?: AbortSignal;
  /** The message of a 404 answer. */
  notFound?: string;
}

/** OpenCode's HTTP API in one environment, with its password and nothing of the browser's request. */
class OpenCode {
  readonly headers: Record<string, string>;

  constructor(
    private readonly url: string,
    password: string,
  ) {
    this.headers = { authorization: `Basic ${btoa(`opencode:${password}`)}` };
  }

  async request(method: string, path: string, options: RequestOptions = {}): Promise<Response> {
    const { body, signal = AbortSignal.timeout(timeoutMs), notFound = "not found" } = options;
    let response: Response;
    try {
      response = await fetch(`${this.url}${path}`, {
        method,
        headers:
          body === undefined
            ? this.headers
            : { ...this.headers, "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: "manual",
        signal,
      });
    } catch (error) {
      throw new OpenCodeError(`OpenCode ${method} ${path}: ${(error as Error).message}`);
    }
    if (response.ok) return response;
    await response.body?.cancel();
    if (response.status === 404) throw new HttpError(404, "not_found", notFound);
    if (response.status === 409) {
      throw new HttpError(409, "conflict", "OpenCode refused the request in the session's state");
    }
    throw new OpenCodeError(`OpenCode ${method} ${path}: ${response.status}`);
  }

  async json(method: string, path: string, options?: RequestOptions) {
    const response = await this.request(method, path, options);
    const bytes = await readAtMost(response.body, answerLimit);
    try {
      return object(JSON.parse(new TextDecoder().decode(bytes ?? new Uint8Array())));
    } catch {
      throw unexpected();
    }
  }

  /** Runs a request whose answer does not matter beyond its status. */
  async send(method: string, path: string, options?: RequestOptions): Promise<void> {
    await (await this.request(method, path, options)).body?.cancel();
  }

  async socket(path: string, signal: AbortSignal): Promise<UpstreamSocket> {
    const url = `${this.url.replace(/^http/, "ws")}${path}`;
    try {
      return await UpstreamSocket.connect(url, this.headers, upstreamFrameLimit, timeoutMs, signal);
    } catch (error) {
      throw new OpenCodeError(
        `OpenCode WebSocket ${path} did not open: ${(error as Error).message}`,
      );
    }
  }
}

function toSession(value: unknown, running: Record<string, unknown>): AgentSession {
  const session = object(value);
  const id = string(session.id, ids.session);
  const time = object(session.time);
  return {
    id,
    title: session.title === undefined ? null : string(session.title),
    createdAt: date(time.created),
    updatedAt: date(time.updated),
    running: Object.hasOwn(running, id),
  };
}

function toPermission(value: unknown): Permission {
  const request = object(value);
  return {
    id: string(request.id, ids.permission),
    sessionId: string(request.sessionID, ids.session),
    action: string(request.action),
    resources: array(request.resources).map((resource) => string(resource)),
  };
}

/** A path relative to /workspace without empty, `.` or `..` segments; "" is the workspace. */
function workspacePath(path: string): string {
  const segments = path === "" ? [] : path.split("/");
  if (
    path.length > 4096 ||
    segments.some((segment) => ["", ".", ".."].includes(segment) || /\p{Cc}/u.test(segment))
  ) {
    throw new HttpError(
      400,
      "invalid_request",
      "path must be relative to /workspace, without empty, '.' or '..' segments",
    );
  }
  return path;
}

function toEntry(value: unknown): FileEntry[] {
  const entry = object(value);
  const type = string(entry.type);
  const path = string(entry.path).replace(/\/$/, "");
  try {
    if ((type === "file" || type === "directory") && workspacePath(path) !== "") {
      return [{ path, type }];
    }
  } catch {}
  return [];
}

// The OpenCode events the stream relays, each reduced to the fields the UI gets;
// every other event is dropped.
type Fields<T, E = AgentEvent> = E extends { type: infer U }
  ? T extends U
    ? Omit<E, "type">
    : never
  : never;
type Data = Record<string, unknown>;
const session = (data: Data) => ({ sessionId: string(data.sessionID, ids.session) });
const message = (data: Data) => ({ ...session(data), messageId: string(data.assistantMessageID) });
const call = (data: Data) => ({ ...message(data), callId: string(data.id) });
const error = (value: unknown) => string(object(value).message);
const relayed: { [T in AgentEvent["type"]]?: (data: Data) => Fields<T> } = {
  "session.created": session,
  "session.deleted": session,
  "session.renamed": (data) => ({ ...session(data), title: string(data.title) }),
  "session.execution.started": session,
  "session.execution.succeeded": session,
  "session.execution.interrupted": session,
  "session.execution.failed": (data) => ({ ...session(data), error: error(data.error) }),
  "session.text.delta": (data) => ({ ...message(data), delta: string(data.delta) }),
  "session.text.ended": (data) => ({ ...message(data), text: string(data.text) }),
  "session.tool.input.started": (data) => ({ ...call(data), name: string(data.name) }),
  "session.tool.called": (data) => ({ ...call(data), input: object(data.input) }),
  "session.tool.success": (data) => ({
    ...call(data),
    output: array(data.content)
      .map((part) => (isObject(part) && part.type === "text" ? string(part.text) : ""))
      .join(""),
  }),
  "session.tool.failed": (data) => ({ ...call(data), error: error(data.error) }),
  "permission.asked": toPermission,
  "permission.replied": (data) => {
    const reply = string(data.reply) as PermissionReply;
    if (!replies.includes(reply)) throw unexpected();
    return { ...session(data), permissionId: string(data.requestID, ids.permission), reply };
  },
};

/** The UI's form of one upstream event's data, or null if it is not relayed or not valid. */
function toEvent(data: string): AgentEvent | null {
  try {
    const event = object(JSON.parse(data));
    const type = string(event.type) as AgentEvent["type"];
    const map = Object.hasOwn(relayed, type) ? relayed[type] : undefined;
    return map === undefined ? null : ({ type, ...map(object(event.data)) } as AgentEvent);
  } catch {
    return null;
  }
}

const sse = (event: AgentEvent) => `data: ${JSON.stringify(event)}\n\n`;

/** The browser's end of a terminal (Bun's ServerWebSocket). */
export interface TerminalSocket {
  readonly readyState: number;
  send(data: string): number;
  close(code?: number, reason?: string): void;
}

/**
 * A shell in the environment, from OpenCode's PTY API, relayed between the
 * browser's WebSocket and OpenCode's. It is removed when either side closes.
 */
export class Terminal {
  private socket: TerminalSocket | null = null;
  private closed: [code: number, reason: string] | null = null;
  // Connecting closes the terminal at once if the environment stopped meanwhile.
  private release = () => {};
  private readonly revoked = () => this.close(1008, "the login session ended");

  constructor(
    private readonly environments: Environments,
    private readonly environmentId: string,
    private readonly opencode: OpenCode,
    private readonly ptyId: string,
    private readonly upstream: UpstreamSocket,
    private readonly access: SessionAccess,
  ) {
    this.release = environments.connect(environmentId, {
      terminal: true,
      close: () => this.close(1000, "the environment stopped"),
    });
    if (this.closed === null) {
      access.signal.addEventListener("abort", this.revoked, { once: true });
      if (!access.check()) this.revoked();
    }
  }

  /** The browser's WebSocket is open; the output flows from now on. */
  open(socket: TerminalSocket): void {
    this.socket = socket;
    if (!this.access.check() && this.closed === null) this.revoked();
    if (this.closed !== null) {
      socket.close(...this.closed);
      return;
    }
    this.upstream.start(
      (output) => this.output(socket, output),
      (failed) =>
        failed
          ? this.close(1011, "OpenCode sent an invalid terminal frame")
          : this.close(1000, "the shell exited"),
    );
  }

  /** A frame from the browser: a JSON TerminalMessage. */
  message(frame: string | Buffer): void {
    if (this.closed !== null || !this.access.check()) return;
    const message = terminalMessage(frame);
    if (message === null) {
      this.close(1008, "expected a JSON input or resize message");
    } else if (message.type === "input") {
      if (message.data !== "" && !this.access.check(true)) return;
      this.upstream.send(message.data);
      this.environments.touch(this.environmentId);
    } else {
      if (!this.access.check(true)) return;
      const size = { rows: message.rows, cols: message.cols };
      this.opencode
        .send("PUT", `/api/pty/${this.ptyId}`, { body: { size } })
        .catch((error) => console.error(`terminal resize failed: ${error.message}`));
    }
  }

  // In frames within the limit, without splitting a surrogate pair.
  private output(socket: TerminalSocket, output: string): void {
    if (this.closed !== null || !this.access.check()) return;
    for (let start = 0; start < output.length; ) {
      let end = Math.min(start + outputFrameChars, output.length);
      const last = output.charCodeAt(end - 1);
      if (end < output.length && last >= 0xd800 && last <= 0xdbff) end--;
      socket.send(output.slice(start, end));
      start = end;
    }
  }

  close(code = 1000, reason = ""): void {
    if (this.closed !== null) return;
    this.closed = [code, reason];
    this.access.signal.removeEventListener("abort", this.revoked);
    this.access.release();
    this.release();
    this.upstream.close();
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.close(code, reason);
    this.opencode.send("DELETE", `/api/pty/${this.ptyId}`).catch(() => {});
  }
}

function terminalMessage(frame: string | Buffer): TerminalMessage | null {
  const size = (value: unknown) =>
    Number.isInteger(value) && Number(value) >= 1 && Number(value) <= 1000;
  try {
    const value = typeof frame === "string" ? object(JSON.parse(frame)) : null;
    if (value?.type === "input" && typeof value.data === "string") {
      return { type: "input", data: value.data };
    }
    if (value?.type === "resize" && size(value.rows) && size(value.cols)) {
      return { type: "resize", rows: Number(value.rows), cols: Number(value.cols) };
    }
  } catch {}
  return null;
}

/**
 * The owner's view of OpenCode in an environment. Every method takes the
 * calling user's id: another user's environment answers 404, one that does
 * not run 409. Prompts, approvals, file reads, relayed events and terminal
 * input count as activity.
 */
export class Agent {
  constructor(private readonly environments: Environments) {}

  async sessions(ownerId: string, id: string): Promise<AgentSession[]> {
    const opencode = await this.opencode(ownerId, id);
    const [list, active] = await Promise.all([
      opencode.json("GET", "/api/session?parentID=null"),
      opencode.json("GET", "/api/session/active"),
    ]);
    const running = object(active.data);
    return array(list.data).map((value) => toSession(value, running));
  }

  async createSession(ownerId: string, id: string): Promise<AgentSession> {
    const opencode = await this.opencode(ownerId, id);
    return toSession((await opencode.json("POST", "/api/session", { body: {} })).data, {});
  }

  async prompt(
    ownerId: string,
    id: string,
    sessionId: string,
    text: string,
  ): Promise<PromptResponse> {
    if (text.trim() === "" || text.length > promptLimit) {
      throw new HttpError(400, "invalid_request", `text must have 1-${promptLimit} characters`);
    }
    const opencode = await this.opencode(ownerId, id);
    const answer = await opencode.json("POST", `/api/session/${sessionPath(sessionId)}/prompt`, {
      body: { text },
      notFound: "no such session",
    });
    this.environments.touch(id);
    return { messageId: string(object(answer.data).id) };
  }

  async abort(ownerId: string, id: string, sessionId: string): Promise<AbortResponse> {
    const opencode = await this.opencode(ownerId, id);
    const answer = await opencode.json("POST", `/api/session/${sessionPath(sessionId)}/interrupt`, {
      notFound: "no such session",
    });
    if (typeof answer.interrupted !== "boolean") throw unexpected();
    return { aborted: answer.interrupted };
  }

  async permissions(ownerId: string, id: string): Promise<Permission[]> {
    const opencode = await this.opencode(ownerId, id);
    return array((await opencode.json("GET", "/api/permission/request")).data).map(toPermission);
  }

  async reply(
    ownerId: string,
    id: string,
    sessionId: string,
    permissionId: string,
    reply: PermissionReply,
  ): Promise<void> {
    if (!replies.includes(reply)) {
      throw new HttpError(400, "invalid_request", "reply must be once, always or reject");
    }
    const opencode = await this.opencode(ownerId, id);
    const notFound = "no such permission request";
    if (!ids.permission.test(permissionId)) throw new HttpError(404, "not_found", notFound);
    await opencode.send(
      "POST",
      `/api/session/${sessionPath(sessionId)}/permission/${permissionId}/reply`,
      { body: { decision: reply }, notFound },
    );
    this.environments.touch(id);
  }

  /** The entries of a directory in /workspace. */
  async files(ownerId: string, id: string, path: string): Promise<FileEntry[]> {
    const query = workspacePath(path) === "" ? "" : `?${new URLSearchParams({ path })}`;
    const opencode = await this.opencode(ownerId, id);
    const answer = await opencode.json("GET", `/api/fs/list${query}`, {
      notFound: "no such directory",
    });
    this.environments.touch(id);
    return array(answer.data).flatMap(toEntry);
  }

  /** The content of a file in /workspace, up to fileLimit bytes. */
  async file(ownerId: string, id: string, path: string): Promise<Uint8Array> {
    if (workspacePath(path) === "") throw new HttpError(400, "invalid_request", "path is required");
    const opencode = await this.opencode(ownerId, id);
    const encoded = path.split("/").map(encodeURIComponent).join("/");
    const response = await opencode.request("GET", `/api/fs/read/${encoded}`, {
      notFound: "no such file",
    });
    const content = await readAtMost(response.body, fileLimit);
    if (content === null) {
      throw new HttpError(413, "too_large", `the file is larger than ${fileLimit} bytes`);
    }
    this.environments.touch(id);
    return content;
  }

  /**
   * OpenCode's event stream as server-sent events: the relayed types only,
   * validated and reduced to their fields. It ends with an
   * `environment.stopped` event when the environment stops.
   */
  async events(
    ownerId: string,
    id: string,
    access: SessionAccess,
  ): Promise<ReadableStream<Uint8Array>> {
    const opencode = await this.opencode(ownerId, id);
    requireAccess(access);
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), timeoutMs);
    const upstream = await opencode
      .request("GET", "/api/event", { signal: AbortSignal.any([abort.signal, access.signal]) })
      .finally(() => clearTimeout(timer));
    if (upstream.body === null) throw unexpected();
    const reader = upstream.body.getReader();
    const decoder = new TextDecoder();
    const encoder = new TextEncoder();
    let buffer = "";
    let skipping = false;
    let ended = false;
    let release = () => {};
    let revoked = () => {};
    const cleanup = () => {
      access.signal.removeEventListener("abort", revoked);
      access.release();
      release();
      abort.abort();
      void reader.cancel().catch(() => {});
    };

    const end = (controller: ReadableStreamDefaultController<Uint8Array>, last?: AgentEvent) => {
      if (ended) return;
      ended = true;
      cleanup();
      if (last !== undefined) controller.enqueue(encoder.encode(sse(last)));
      controller.close();
    };
    // Reads upstream only as fast as the browser takes the events.
    return new ReadableStream<Uint8Array>({
      start: (controller) => {
        // Bun sends the response headers with the first chunk.
        controller.enqueue(encoder.encode(": connected\n\n"));
        release = this.environments.connect(id, {
          terminal: false,
          close: () => end(controller, { type: "environment.stopped" }),
        });
        if (!ended) {
          revoked = () => end(controller);
          access.signal.addEventListener("abort", revoked, { once: true });
          if (!access.check()) end(controller);
        }
      },
      pull: async (controller) => {
        while (!ended) {
          const chunk = await reader.read().catch(() => ({ done: true, value: undefined }));
          if (ended) return;
          if (!access.check()) return end(controller);
          if (chunk.done) return end(controller);
          buffer += decoder.decode(chunk.value, { stream: true }).replaceAll("\r", "");
          let output = "";
          for (let index = buffer.indexOf("\n\n"); index !== -1; index = buffer.indexOf("\n\n")) {
            const frame = buffer.slice(0, index);
            buffer = buffer.slice(index + 2);
            if (skipping) skipping = false;
            else if (frame.length <= eventFrameLimit) output += this.relay(id, frame);
          }
          if (buffer.length > eventFrameLimit) {
            buffer = "";
            skipping = true;
          }
          if (output !== "" && !ended) return controller.enqueue(encoder.encode(output));
        }
      },
      cancel: () => {
        ended = true;
        cleanup();
      },
    });
  }

  /** Opens a shell in the environment as its user (1000), in /workspace. */
  async terminal(ownerId: string, id: string, access: SessionAccess): Promise<Terminal> {
    const opencode = await this.opencode(ownerId, id);
    requireAccess(access);
    // Let a pending creation return its ID even after revocation so it can be removed.
    const pty = await opencode.json("POST", "/api/pty", { body: { title: "tentaflake" } });
    const ptyId = string(object(pty.data).id, ids.pty);
    try {
      requireAccess(access);
      const upstream = await opencode.socket(`/api/pty/${ptyId}/connect`, access.signal);
      return new Terminal(this.environments, id, opencode, ptyId, upstream, access);
    } catch (error) {
      opencode.send("DELETE", `/api/pty/${ptyId}`).catch(() => {});
      throw error;
    }
  }

  /** Whether a prompt runs in the environment; false if its container does not run. */
  async busy(ownerId: string, id: string): Promise<boolean> {
    const endpoint = await this.environments.opencode(ownerId, id);
    if (endpoint === null) return false;
    const active = await new OpenCode(endpoint.url, endpoint.password).json(
      "GET",
      "/api/session/active",
    );
    return Object.keys(object(active.data)).length > 0;
  }

  private async opencode(ownerId: string, id: string): Promise<OpenCode> {
    const endpoint = await this.environments.opencode(ownerId, id);
    if (endpoint === null) throw new HttpError(409, "conflict", "the environment is not running");
    return new OpenCode(endpoint.url, endpoint.password);
  }

  /** One upstream frame as the browser gets it: a relayed event, a heartbeat or nothing. */
  private relay(id: string, frame: string): string {
    const data = frame
      .split("\n")
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(line.startsWith("data: ") ? 6 : 5))
      .join("\n");
    if (data === "") return frame.startsWith(":") ? ": heartbeat\n\n" : "";
    const event = toEvent(data);
    if (event === null) return "";
    this.environments.touch(id);
    return sse(event);
  }
}

function sessionPath(sessionId: string): string {
  if (!ids.session.test(sessionId)) throw new HttpError(404, "not_found", "no such session");
  return sessionId;
}

/**
 * Stops running environments without activity for `idleMs`, unless a
 * terminal is open or a prompt runs; a running prompt counts as activity.
 * A stopped environment keeps its workspace and sessions.
 */
export class IdleStop {
  private running = false;

  constructor(
    private readonly environments: Environments,
    private readonly agent: Agent,
    private readonly idleMs: number,
    private readonly now: () => number = Date.now,
  ) {}

  async run(): Promise<void> {
    if (this.running) return;
    this.running = true;
    try {
      for (const { id, ownerId } of this.environments.idleSince(this.now() - this.idleMs)) {
        try {
          if (this.environments.terminalOpen(id)) continue;
          if (await this.agent.busy(ownerId, id)) {
            this.environments.touch(id);
            continue;
          }
          await this.environments.stop(ownerId, id);
          console.log(`environment ${id}: stopped after ${this.idleMs / 60_000} idle minutes`);
        } catch (error) {
          // Unknown means busy: an environment is never stopped on a failed check.
          console.error(`environment ${id}: idle check failed: ${(error as Error).message}`);
        }
      }
    } finally {
      this.running = false;
    }
  }
}
