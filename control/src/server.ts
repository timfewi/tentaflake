import type { BunRequest, Server } from "bun";
import { type Agent, OpenCodeError, type Terminal, terminalFrameLimit } from "./agent";
import {
  type AgentSessionList,
  type CreateEnvironmentRequest,
  type CreateKeyRequest,
  csrfHeader,
  type EnvironmentList,
  type FileEntryList,
  type KeyList,
  type McpServerList,
  type NetworkMode,
  type PermissionList,
  type PermissionReply,
  type Provider,
  type SessionResponse,
  type SetupStatus,
} from "./api";
import type { Auth, Session } from "./auth";
import type { Config } from "./config";
import { safeEqual } from "./crypto";
import { DockerError } from "./docker";
import type { Environments } from "./environments";
import { errorJson, HttpError, json, readBody, text, texts } from "./http";
import type { Keys } from "./keys";

export const sessionCookie = "__Host-tentaflake";

const cookie = (value: string, maxAge: number) =>
  `${sessionCookie}=${value}; Path=/; Max-Age=${maxAge}; Secure; HttpOnly; SameSite=Strict`;

const sessionResponse = ({ user, csrfToken }: Session): SessionResponse => ({ user, csrfToken });

// Undefined after a WebSocket upgrade.
type Reply = Response | undefined | Promise<Response | undefined>;
const notOrigin = () => new HttpError(403, "origin_mismatch", "the request origin is not allowed");
const pathParameter = (request: Request) =>
  text(new URL(request.url).searchParams.get("path") ?? "", "path", 4096);

/** Serves the /api/v1 endpoints listed in docs/api.md. */
export function startServer(
  config: Pick<Config, "hostname" | "port" | "origin">,
  auth: Auth,
  environments: Environments,
  keys: Keys,
  agent: Agent,
) {
  // Every mutation must come from the configured origin exactly.
  function open<P extends string>(
    handler: (request: BunRequest<P>, server: Server<Terminal>) => Reply,
  ) {
    return async (request: BunRequest<P>, server: Server<Terminal>) => {
      try {
        if (request.method !== "GET" && request.headers.get("origin") !== config.origin) {
          throw notOrigin();
        }
        return await handler(request, server);
      } catch (error) {
        return failure(error);
      }
    };
  }

  // Requires a live session, and its CSRF token on every mutation.
  function authed<P extends string>(
    handler: (request: BunRequest<P>, session: Session, server: Server<Terminal>) => Reply,
  ) {
    return open<P>((request, server) => {
      const session = auth.authenticate(request.cookies.get(sessionCookie) ?? undefined);
      if (session === null) throw new HttpError(401, "unauthenticated", "sign in first");
      const csrf = request.headers.get(csrfHeader) ?? "";
      if (request.method !== "GET" && !safeEqual(csrf, session.csrfToken)) {
        throw new HttpError(403, "csrf_mismatch", "the CSRF token is missing or wrong");
      }
      return handler(request, session, server);
    });
  }

  return Bun.serve({
    hostname: config.hostname,
    port: config.port,
    maxRequestBodySize: 64 * 1024,
    websocket: {
      data: {} as Terminal,
      maxPayloadLength: terminalFrameLimit,
      backpressureLimit: 1024 * 1024,
      closeOnBackpressureLimit: true,
      open: (socket) => socket.data.open(socket),
      message: (socket, frame) => socket.data.message(frame),
      close: (socket) => socket.data.close(),
    },
    routes: {
      "/api/v1/setup": {
        GET: open(() => json(200, { needed: auth.setupNeeded() } satisfies SetupStatus)),
        POST: open(async (request) => {
          const body = await readBody(request);
          const setup = await auth.setup({
            token: text(body.token, "token"),
            username: text(body.username, "username"),
            password: text(body.password, "password"),
          });
          return json(201, setup);
        }),
      },
      "/api/v1/session": {
        GET: authed((_, session) => json(200, sessionResponse(session))),
        POST: open(async (request) => {
          const body = await readBody(request);
          const { token, session } = await auth.login({
            username: text(body.username, "username"),
            password: text(body.password, "password"),
            code: text(body.code, "code"),
          });
          return json(200, session, { "set-cookie": cookie(token, auth.sessionSeconds) });
        }),
        DELETE: authed((_, session) => {
          auth.logout(session.tokenHash);
          return new Response(null, { status: 204, headers: { "set-cookie": cookie("", 0) } });
        }),
      },
      "/api/v1/environments": {
        GET: authed((_, { user }) =>
          json(200, { environments: environments.list(user.id) } satisfies EnvironmentList),
        ),
        POST: authed(async (request, { user }) => {
          const body = await readBody(request);
          const create: CreateEnvironmentRequest = {
            name: text(body.name, "name"),
            keyId: text(body.keyId, "keyId"),
            models: texts(body.models, "models"),
          };
          if (body.networkMode !== undefined) {
            create.networkMode = text(body.networkMode, "networkMode") as NetworkMode;
          }
          return json(201, await environments.create(user.id, create));
        }),
      },
      "/api/v1/environments/:id": {
        GET: authed((request, { user }) => json(200, environments.get(user.id, request.params.id))),
        DELETE: authed(async (request, { user }) => {
          await environments.delete(user.id, request.params.id);
          return new Response(null, { status: 204 });
        }),
      },
      "/api/v1/environments/:id/start": {
        POST: authed(async (request, { user }) =>
          json(200, await environments.start(user.id, request.params.id)),
        ),
      },
      "/api/v1/environments/:id/stop": {
        POST: authed(async (request, { user }) =>
          json(200, await environments.stop(user.id, request.params.id)),
        ),
      },
      "/api/v1/environments/:id/mcp": {
        GET: authed((request, { user }) =>
          json(200, {
            servers: environments.mcpServers(user.id, request.params.id),
          } satisfies McpServerList),
        ),
      },
      "/api/v1/environments/:id/mcp/:name": {
        PUT: authed(async (request, { user }) => {
          const { id, name } = request.params;
          return json(
            200,
            await environments.putMcpServer(user.id, id, name, await readBody(request)),
          );
        }),
        DELETE: authed(async (request, { user }) => {
          await environments.deleteMcpServer(user.id, request.params.id, request.params.name);
          return new Response(null, { status: 204 });
        }),
      },
      "/api/v1/environments/:id/sessions": {
        GET: authed(async (request, { user }) =>
          json(200, {
            sessions: await agent.sessions(user.id, request.params.id),
          } satisfies AgentSessionList),
        ),
        POST: authed(async (request, { user }) =>
          json(201, await agent.createSession(user.id, request.params.id)),
        ),
      },
      "/api/v1/environments/:id/sessions/:session/prompt": {
        POST: authed(async (request, { user }) => {
          const { id, session } = request.params;
          const { text: prompt } = await readBody(request);
          const answer = await agent.prompt(
            user.id,
            id,
            session,
            typeof prompt === "string" ? prompt : "",
          );
          return json(202, answer);
        }),
      },
      "/api/v1/environments/:id/sessions/:session/abort": {
        POST: authed(async (request, { user }) =>
          json(200, await agent.abort(user.id, request.params.id, request.params.session)),
        ),
      },
      "/api/v1/environments/:id/sessions/:session/permissions/:permission": {
        POST: authed(async (request, { user }) => {
          const { id, session, permission } = request.params;
          const reply = text((await readBody(request)).reply, "reply") as PermissionReply;
          await agent.reply(user.id, id, session, permission, reply);
          return new Response(null, { status: 204 });
        }),
      },
      "/api/v1/environments/:id/permissions": {
        GET: authed(async (request, { user }) =>
          json(200, {
            permissions: await agent.permissions(user.id, request.params.id),
          } satisfies PermissionList),
        ),
      },
      "/api/v1/environments/:id/events": {
        GET: authed(async (request, { user }, server) => {
          const events = await agent.events(user.id, request.params.id);
          server.timeout(request, 0);
          return new Response(events, {
            headers: { "content-type": "text/event-stream", "cache-control": "no-store" },
          });
        }),
      },
      "/api/v1/environments/:id/files": {
        GET: authed(async (request, { user }) =>
          json(200, {
            entries: await agent.files(user.id, request.params.id, pathParameter(request)),
          } satisfies FileEntryList),
        ),
      },
      "/api/v1/environments/:id/files/content": {
        GET: authed(async (request, { user }) => {
          const content = await agent.file(user.id, request.params.id, pathParameter(request));
          // Bytes from the environment: never rendered or sniffed on this origin.
          return new Response(content, {
            headers: {
              "content-type": "application/octet-stream",
              "content-disposition": "attachment",
              "content-security-policy": "sandbox",
              "x-content-type-options": "nosniff",
              "cache-control": "no-store",
            },
          });
        }),
      },
      "/api/v1/environments/:id/terminal": {
        GET: authed(async (request, { user }, server) => {
          // The WebSocket handshake is a GET, but it opens a shell: Origin is checked.
          if (request.headers.get("origin") !== config.origin) throw notOrigin();
          if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
            throw new HttpError(400, "invalid_request", "expected a WebSocket upgrade");
          }
          const terminal = await agent.terminal(user.id, request.params.id);
          if (server.upgrade(request, { data: terminal })) return undefined;
          terminal.close();
          throw new HttpError(400, "invalid_request", "the WebSocket upgrade failed");
        }),
      },
      "/api/v1/keys": {
        GET: authed((_, { user }) => json(200, { keys: keys.list(user.id) } satisfies KeyList)),
        POST: authed(async (request, { user }) => {
          const body = await readBody(request);
          const create: CreateKeyRequest = {
            provider: text(body.provider, "provider") as Provider,
            label: text(body.label, "label"),
            secret: text(body.secret, "secret", 8192),
          };
          if (body.baseUrl !== undefined) create.baseUrl = text(body.baseUrl, "baseUrl", 4096);
          return json(201, keys.add(user.id, create));
        }),
      },
      "/api/v1/keys/:id": {
        DELETE: authed((request, { user }) => {
          keys.delete(user.id, request.params.id);
          return new Response(null, { status: 204 });
        }),
      },
    },
    fetch: () => errorJson(404, "not_found", "no such endpoint"),
  });
}

function failure(error: unknown): Response {
  if (error instanceof HttpError) return errorJson(error.status, error.code, error.message);
  if (error instanceof DockerError) {
    console.error(error.message);
    return errorJson(502, "docker", "a Docker request failed");
  }
  if (error instanceof OpenCodeError) {
    console.error(error.message);
    return errorJson(502, "opencode", "OpenCode in the environment did not answer as expected");
  }
  console.error(error);
  return errorJson(500, "internal", "internal error");
}
