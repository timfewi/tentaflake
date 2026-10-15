// Request and response types of the control service API (/api/v1), shared
// with the UI. docs/api.md lists the endpoints.

export type Role = "admin" | "member";
export type NetworkMode = "model-only" | "open";
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
}

export interface Environment {
  id: string;
  name: string;
  state: EnvironmentState;
  networkMode: NetworkMode;
  limits: Limits;
  createdAt: string;
  lastActivityAt: string;
}

export interface CreateEnvironmentRequest {
  name: string;
  /** Defaults to "model-only". */
  networkMode?: NetworkMode;
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
