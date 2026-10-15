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

export type ErrorCode =
  | "invalid_request"
  | "unsupported_media_type"
  | "unauthenticated"
  | "invalid_credentials"
  | "origin_mismatch"
  | "csrf_mismatch"
  | "setup_unavailable"
  | "invalid_setup_token"
  | "setup_complete"
  | "not_found"
  | "conflict"
  | "rate_limited"
  | "docker"
  | "internal";

export interface ApiError {
  error: ErrorCode;
  message: string;
}
