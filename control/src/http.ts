import type { ApiError, ErrorCode } from "./api";
import { isObject } from "./config";

/** An error with its API status and code; the message is shown to the client. */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: ErrorCode,
    message: string,
  ) {
    super(message);
  }
}

export function json(
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
): Response {
  return Response.json(body, { status, headers: { "cache-control": "no-store", ...headers } });
}

export function errorJson(status: number, code: ErrorCode, message: string): Response {
  return json(status, { error: code, message } satisfies ApiError);
}

/** The JSON object in a request body. */
export async function readBody(request: Request): Promise<Record<string, unknown>> {
  const type = request.headers.get("content-type") ?? "";
  if (!/^application\/json\s*(;|$)/i.test(type)) {
    throw new HttpError(415, "unsupported_media_type", "the request body must be application/json");
  }
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    throw new HttpError(400, "invalid_request", "the request body is not valid JSON");
  }
  if (!isObject(body)) {
    throw new HttpError(400, "invalid_request", "the request body must be an object");
  }
  return body;
}

/** A list of strings, e.g. model names. */
export function texts(value: unknown, field: string, maxItems = 64): string[] {
  if (!Array.isArray(value) || value.length > maxItems) {
    throw new HttpError(400, "invalid_request", `${field} must be a list of at most ${maxItems}`);
  }
  return value.map((item, index) => text(item, `${field}[${index}]`));
}

export function text(value: unknown, field: string, maxLength = 1024): string {
  if (typeof value !== "string" || value.length > maxLength) {
    throw new HttpError(
      400,
      "invalid_request",
      `${field} must be a string of at most ${maxLength} characters`,
    );
  }
  return value;
}
