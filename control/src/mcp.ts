import type { McpServerDefinition } from "./api";
import { isObject } from "./config";
import { HttpError } from "./http";

export const mcpNamePattern = /^[a-z0-9][a-z0-9_-]{0,62}$/;

/** Returned with every secret assignment. */
export const mcpSecretWarning =
  "The agent in this environment can read these secrets: OpenCode receives them in its configuration.";

const invalid = (message: string) => new HttpError(400, "invalid_request", message);

const keys = {
  local: { list: "command", map: "environment", names: /^[A-Za-z_][A-Za-z0-9_]{0,127}$/ },
  remote: { list: "url", map: "headers", names: /^[A-Za-z0-9-]{1,128}$/ },
} as const;

/**
 * Validates a PUT body into the definition stored as plain JSON and the
 * secrets stored encrypted: environment variables of a local server or
 * headers of a remote one.
 */
export function parseMcpServer(body: Record<string, unknown>): {
  definition: McpServerDefinition;
  secrets: Record<string, string>;
} {
  const type = body.type;
  if (type !== "local" && type !== "remote") throw invalid("type must be local or remote");
  const { list, map, names } = keys[type];
  for (const key of Object.keys(body)) {
    if (!["type", list, map, "secrets"].includes(key)) {
      throw invalid(`unknown field for a ${type} server: ${key}`);
    }
  }
  const plain = stringMap(body[map], map, names);
  const secrets = stringMap(body.secrets, "secrets", names);
  if (type === "local") {
    const command = body.command;
    if (
      !Array.isArray(command) ||
      command.length === 0 ||
      command.length > 64 ||
      !command.every((part) => typeof part === "string" && part !== "" && part.length <= 4096)
    ) {
      throw invalid("command must be a list of 1-64 non-empty strings");
    }
    return { definition: { type, command, environment: plain }, secrets };
  }
  const url = typeof body.url === "string" && URL.canParse(body.url) ? new URL(body.url) : null;
  if (url === null || !["http:", "https:"].includes(url.protocol) || url.href.length > 2048) {
    throw invalid("url must be an http(s) URL");
  }
  return { definition: { type, url: url.href, headers: plain }, secrets };
}

/** The server as OpenCode's `mcp` configuration entry, secrets merged in. */
export function opencodeMcp(definition: McpServerDefinition, secrets: Record<string, string>) {
  return definition.type === "local"
    ? { ...definition, environment: { ...definition.environment, ...secrets } }
    : { ...definition, headers: { ...definition.headers, ...secrets } };
}

function stringMap(value: unknown, field: string, names: RegExp): Record<string, string> {
  if (value === undefined) return {};
  if (!isObject(value) || Object.keys(value).length > 64) {
    throw invalid(`${field} must be an object of at most 64 entries`);
  }
  for (const [name, item] of Object.entries(value)) {
    if (!names.test(name)) throw invalid(`${field} has an invalid name: ${name}`);
    if (
      typeof item !== "string" ||
      item.length > 4096 ||
      /\p{Cc}/u.test(item.replaceAll("\t", ""))
    ) {
      throw invalid(`${field}.${name} must be a string of at most 4096 characters`);
    }
  }
  return value as Record<string, string>;
}
