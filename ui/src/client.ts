import { csrfHeader, type ErrorCode } from "../../control/src/api";
import type * as Api from "../../control/src/api";

export type * from "../../control/src/api";

/** A failed API call; `network` means the service could not be reached at all. */
export class ApiFailure extends Error {
  constructor(
    readonly status: number,
    readonly code: ErrorCode | "network",
    message: string,
  ) {
    super(message);
  }
}

export const errorMessage = (error: unknown) =>
  error instanceof Error ? error.message : "something went wrong";

let csrfToken = "";
let onSignedOut = () => {};
/** Called when the service reports that the session ended. */
export const whenSignedOut = (callback: () => void) => {
  onSignedOut = callback;
};

async function send(method: string, path: string, body?: unknown): Promise<Response> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  if (method !== "GET") headers[csrfHeader] = csrfToken;
  let response: Response;
  try {
    response = await fetch(`/api/v1${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiFailure(0, "network", "the service is not reachable");
  }
  if (response.ok) return response;
  const failure = (await response.json().catch(() => null)) as {
    error?: ErrorCode;
    message?: string;
  } | null;
  const error = new ApiFailure(
    response.status,
    failure?.error ?? "internal",
    failure?.message ?? `the service answered ${response.status}`,
  );
  if (error.code === "unauthenticated") onSignedOut();
  throw error;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await send(method, path, body);
  return (response.status === 204 ? undefined : await response.json()) as T;
}

const remember = (session: Api.SessionResponse) => {
  csrfToken = session.csrfToken;
  return session;
};

const environment = (id: string) => `/environments/${id}`;
const session = (id: string, sessionId: string) => `${environment(id)}/sessions/${sessionId}`;

/** The control service's /api/v1, typed by control/src/api.ts. */
export const api = {
  setupStatus: () => request<Api.SetupStatus>("GET", "/setup"),
  setup: (body: Api.SetupRequest) => request<Api.SetupResponse>("POST", "/setup", body),
  login: async (body: Api.LoginRequest) =>
    remember(await request<Api.SessionResponse>("POST", "/session", body)),
  session: async () => remember(await request<Api.SessionResponse>("GET", "/session")),
  logout: () => request<void>("DELETE", "/session"),

  keys: async () => (await request<Api.KeyList>("GET", "/keys")).keys,
  addKey: (body: Api.CreateKeyRequest) => request<Api.ModelKey>("POST", "/keys", body),
  deleteKey: (id: string) => request<void>("DELETE", `/keys/${id}`),

  environments: async () =>
    (await request<Api.EnvironmentList>("GET", "/environments")).environments,
  environment: (id: string) => request<Api.Environment>("GET", environment(id)),
  createEnvironment: (body: Api.CreateEnvironmentRequest) =>
    request<Api.Environment>("POST", "/environments", body),
  startEnvironment: (id: string) => request<Api.Environment>("POST", `${environment(id)}/start`),
  stopEnvironment: (id: string) => request<Api.Environment>("POST", `${environment(id)}/stop`),
  deleteEnvironment: (id: string) => request<void>("DELETE", environment(id)),
  putMcpServer: (id: string, name: string, body: Api.McpServerRequest) =>
    request<Api.McpServerResponse>(
      "PUT",
      `${environment(id)}/mcp/${encodeURIComponent(name)}`,
      body,
    ),

  sessions: async (id: string) =>
    (await request<Api.AgentSessionList>("GET", `${environment(id)}/sessions`)).sessions,
  createSession: (id: string) => request<Api.AgentSession>("POST", `${environment(id)}/sessions`),
  prompt: (id: string, sessionId: string, text: string) =>
    request<Api.PromptResponse>("POST", `${session(id, sessionId)}/prompt`, { text }),
  abort: (id: string, sessionId: string) =>
    request<Api.AbortResponse>("POST", `${session(id, sessionId)}/abort`),
  permissions: async (id: string): Promise<Api.Permission[]> =>
    (await request<Api.PermissionList>("GET", `${environment(id)}/permissions`)).permissions,
  reply: (id: string, sessionId: string, permissionId: string, reply: Api.PermissionReply) =>
    request<void>("POST", `${session(id, sessionId)}/permissions/${permissionId}`, { reply }),

  files: async (id: string, path: string): Promise<Api.FileEntry[]> =>
    (
      await request<Api.FileEntryList>(
        "GET",
        `${environment(id)}/files?path=${encodeURIComponent(path)}`,
      )
    ).entries,
  fileContent: async (id: string, path: string) =>
    new Uint8Array(
      await (
        await send("GET", `${environment(id)}/files/content?path=${encodeURIComponent(path)}`)
      ).arrayBuffer(),
    ),
};

/** Same-origin URLs of the streams; the session cookie authenticates both. */
export const eventsUrl = (id: string) => `/api/v1${environment(id)}/events`;
export const terminalUrl = (id: string) =>
  `${location.origin.replace(/^http/, "ws")}/api/v1${environment(id)}/terminal`;
