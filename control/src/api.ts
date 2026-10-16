// Request and response types of the control service API (/api/v1), shared
// with the UI. docs/api.md lists the endpoints.

export type Role = "admin" | "member";
export type NetworkMode = "model-only" | "open";
export type Provider = "anthropic" | "openai" | "openrouter" | "openai-compatible";
export type EnvironmentState = "creating" | "running" | "stopped" | "failed";

/** Header that carries the session's CSRF token on every authenticated mutation. */
export const csrfHeader = "x-csrf-token";

export interface User {
  id: string;
  username: string;
  role: Role;
  createdAt: string;
}

export interface SetupStatus {
  /** True while no user exists and the first admin can still be created. */
  needed: boolean;
}

export interface SetupRequest {
  token: string;
  username: string;
  password: string;
}

export interface SetupResponse {
  user: User;
  /** The TOTP enrollment URI. It is returned once and never again. */
  otpauthUri: string;
}

/** The domain the service answers on; GET and PUT /api/v1/domain. */
export interface DomainStatus {
  domain: string;
  /** True when it was chosen in setup, false for the name derived from the server's address. */
  custom: boolean;
  /** The server's public IPv4 address, which a chosen domain must resolve to; null if unknown. */
  address: string | null;
  /** False when the service configuration fixes the origin. */
  configurable: boolean;
}

export interface DomainRequest {
  /** A host name such as agents.example.com; null or empty returns to the derived name. */
  domain: string | null;
}

export interface LoginRequest {
  username: string;
  password: string;
  code: string;
}

export interface SessionResponse {
  user: User;
  csrfToken: string;
}

export interface Limits {
  memoryMiB: number;
  cpus: number;
  pids: number;
  /** The broker's daily token budget for the environment. */
  dailyTokens: number;
}

/** A model API key; its secret is never returned. */
export interface ModelKey {
  id: string;
  provider: Provider;
  label: string;
  /** The API base URL of an openai-compatible key, otherwise null. */
  baseUrl: string | null;
  createdAt: string;
}

export interface CreateKeyRequest {
  provider: Provider;
  label: string;
  /** The provider API key; stored encrypted and used only by the broker. */
  secret: string;
  /** Required for openai-compatible, e.g. https://llm.example/v1. */
  baseUrl?: string;
}

export interface KeyList {
  keys: ModelKey[];
}

export interface Environment {
  id: string;
  name: string;
  state: EnvironmentState;
  networkMode: NetworkMode;
  /** The caller's key the environment's model calls use; null after the key was deleted. */
  keyId: string | null;
  models: string[];
  limits: Limits;
  createdAt: string;
  lastActivityAt: string;
}

export interface CreateEnvironmentRequest {
  name: string;
  /** Defaults to "model-only". */
  networkMode?: NetworkMode;
  /** One of the caller's keys. */
  keyId: string;
  /** The models the environment may use; the first is OpenCode's default. */
  models: string[];
}

export type McpServerDefinition =
  | { type: "local"; command: string[]; environment?: Record<string, string> }
  | { type: "remote"; url: string; headers?: Record<string, string> };

export type McpServerRequest = McpServerDefinition & {
  /** Environment variables (local) or headers (remote) whose values the API never returns. */
  secrets?: Record<string, string>;
};

export type McpServer = McpServerDefinition & {
  name: string;
  /** The names of the assigned secrets. */
  secrets: string[];
};

export interface McpServerList {
  servers: McpServer[];
}

export interface McpServerResponse {
  server: McpServer;
  /** Present when secrets were assigned: the agent in the environment can read them. */
  warning?: string;
}

export interface EnvironmentList {
  environments: Environment[];
}

/** An OpenCode session in an environment. */
export interface AgentSession {
  id: string;
  title: string | null;
  createdAt: string;
  updatedAt: string;
  /** True while a prompt runs. */
  running: boolean;
}

export interface AgentSessionList {
  sessions: AgentSession[];
}

export interface PromptRequest {
  text: string;
}

export interface PromptResponse {
  /** The id of the user message OpenCode recorded. */
  messageId: string;
}

export interface AbortResponse {
  /** False when no prompt was running. */
  aborted: boolean;
}

/** A tool call that waits for the owner's approval (an OpenCode `ask` permission). */
export interface Permission {
  id: string;
  sessionId: string;
  /** The permission, e.g. "shell" or "webfetch". */
  action: string;
  /** What the tool wants to act on, e.g. the shell commands. */
  resources: string[];
}

export interface PermissionList {
  permissions: Permission[];
}

/** Allow this call, allow calls like it from now on, or refuse it. */
export type PermissionReply = "once" | "always" | "reject";

export interface PermissionReplyRequest {
  reply: PermissionReply;
}

export interface FileEntry {
  /** Relative to /workspace. */
  path: string;
  type: "file" | "directory";
}

export interface FileEntryList {
  entries: FileEntry[];
}

/** One event of GET /api/v1/environments/{id}/events, sent as the `data` of a server-sent event. */
export type AgentEvent =
  | {
      type:
        | "session.created"
        | "session.deleted"
        | "session.execution.started"
        | "session.execution.succeeded"
        | "session.execution.interrupted";
      sessionId: string;
    }
  | { type: "session.renamed"; sessionId: string; title: string }
  | { type: "session.execution.failed"; sessionId: string; error: string }
  | { type: "session.text.delta"; sessionId: string; messageId: string; delta: string }
  | { type: "session.text.ended"; sessionId: string; messageId: string; text: string }
  | {
      type: "session.tool.input.started";
      sessionId: string;
      messageId: string;
      callId: string;
      name: string;
    }
  | {
      type: "session.tool.called";
      sessionId: string;
      messageId: string;
      callId: string;
      input: Record<string, unknown>;
    }
  | {
      type: "session.tool.success";
      sessionId: string;
      messageId: string;
      callId: string;
      output: string;
    }
  | {
      type: "session.tool.failed";
      sessionId: string;
      messageId: string;
      callId: string;
      error: string;
    }
  | ({ type: "permission.asked" } & Permission)
  | { type: "permission.replied"; sessionId: string; permissionId: string; reply: PermissionReply }
  /** The last event: the environment stopped and the stream ends. */
  | { type: "environment.stopped" };

/**
 * A text frame from the browser on the terminal WebSocket. The service sends
 * the terminal's output back as text frames.
 */
export type TerminalMessage =
  | { type: "input"; data: string }
  | { type: "resize"; rows: number; cols: number };

export type ErrorCode =
  | "invalid_request"
  | "unsupported_media_type"
  | "unauthenticated"
  | "invalid_credentials"
  | "origin_mismatch"
  | "csrf_mismatch"
  | "forbidden"
  | "setup_unavailable"
  | "invalid_setup_token"
  | "setup_complete"
  | "not_found"
  | "conflict"
  | "too_large"
  | "rate_limited"
  | "unavailable"
  | "docker"
  | "opencode"
  | "internal";

export interface ApiError {
  error: ErrorCode;
  message: string;
}
