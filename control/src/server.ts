import type { BunRequest } from "bun";
import {
  type CreateEnvironmentRequest,
  type CreateKeyRequest,
  csrfHeader,
  type EnvironmentList,
  type KeyList,
  type McpServerList,
  type NetworkMode,
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

type Reply = Response | Promise<Response>;

/** Serves the /api/v1 endpoints listed in docs/api.md. */
export function startServer(
  config: Pick<Config, "hostname" | "port" | "origin">,
  auth: Auth,
  environments: Environments,
  keys: Keys,
) {
  // Every mutation must come from the configured origin exactly.
  function open<P extends string>(handler: (request: BunRequest<P>) => Reply) {
    return async (request: BunRequest<P>): Promise<Response> => {
      try {
        if (request.method !== "GET" && request.headers.get("origin") !== config.origin) {
          throw new HttpError(403, "origin_mismatch", "the request origin is not allowed");
        }
        return await handler(request);
      } catch (error) {
        return failure(error);
      }
    };
  }

  // Requires a live session, and its CSRF token on every mutation.
  function authed<P extends string>(handler: (request: BunRequest<P>, session: Session) => Reply) {
    return open<P>((request) => {
      const session = auth.authenticate(request.cookies.get(sessionCookie) ?? undefined);
      if (session === null) throw new HttpError(401, "unauthenticated", "sign in first");
      const csrf = request.headers.get(csrfHeader) ?? "";
      if (request.method !== "GET" && !safeEqual(csrf, session.csrfToken)) {
        throw new HttpError(403, "csrf_mismatch", "the CSRF token is missing or wrong");
      }
      return handler(request, session);
    });
  }

  return Bun.serve({
    hostname: config.hostname,
    port: config.port,
    maxRequestBodySize: 64 * 1024,
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
  console.error(error);
  return errorJson(500, "internal", "internal error");
}
