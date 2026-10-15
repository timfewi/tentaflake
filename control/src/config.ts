import { readFileSync } from "node:fs";
import type { Limits } from "./api";

/** The service configuration: one JSON file without secrets. */
export interface Config {
  hostname: string;
  port: number;
  /** The exact browser origin, e.g. https://tentaflake.example; mutations must carry it. */
  origin: string;
  databasePath: string;
  /** 32 random bytes that encrypt the secrets in the database. */
  keyFile: string;
  /** One-time token that authorises creating the first admin. */
  setupTokenFile: string;
  dockerSocket: string;
  /** The broker's port on the gateway address of the model-only network. */
  brokerPort: number;
  /** Runtime directory (tmpfs) for the broker's routes and provider credential files. */
  brokerDirectory: string;
  /** The runtime image reference, ideally by digest. */
  image: string;
  limits: Limits;
  /** Minutes without activity after which a running environment is stopped. */
  environmentIdleMinutes: number;
  sessionIdleMinutes: number;
  sessionAbsoluteHours: number;
}

const defaults = {
  hostname: "127.0.0.1",
  port: 8080,
  dockerSocket: "/run/docker.sock",
  brokerPort: 7810,
  brokerDirectory: "/run/tentaflake/broker",
  limits: { memoryMiB: 2048, cpus: 2, pids: 512, dailyTokens: 10_000_000 },
  environmentIdleMinutes: 15,
  sessionIdleMinutes: 60,
  sessionAbsoluteHours: 12,
};
const required = ["origin", "databasePath", "keyFile", "setupTokenFile", "image"] as const;

export function loadConfig(path: string): Config {
  return parseConfig(JSON.parse(readFileSync(path, "utf8")));
}

export function parseConfig(input: unknown): Config {
  if (!isObject(input)) throw new Error("the configuration must be a JSON object");
  for (const key of Object.keys(input)) {
    if (!(key in defaults) && !(required as readonly string[]).includes(key)) {
      throw new Error(`unknown configuration key: ${key}`);
    }
  }
  const limits = { ...defaults.limits, ...(isObject(input.limits) ? input.limits : {}) };
  const config = { ...defaults, ...input, limits } as Record<keyof Config, unknown>;

  for (const key of [...required, "hostname", "dockerSocket", "brokerDirectory"] as const) {
    if (typeof config[key] !== "string" || config[key] === "") {
      throw new Error(`configuration key ${key} must be a non-empty string`);
    }
  }
  const origin = config.origin as string;
  if (!URL.canParse(origin) || new URL(origin).origin !== origin) {
    throw new Error(
      "configuration key origin must be an origin such as https://tentaflake.example",
    );
  }
  integer(config.port, "port", 0, 65535);
  integer(config.brokerPort, "brokerPort", 1, 65535);
  integer(config.environmentIdleMinutes, "environmentIdleMinutes", 1);
  integer(config.sessionIdleMinutes, "sessionIdleMinutes", 1);
  integer(config.sessionAbsoluteHours, "sessionAbsoluteHours", 1);
  for (const key of Object.keys(limits)) {
    if (!(key in defaults.limits)) throw new Error(`unknown configuration key: limits.${key}`);
  }
  integer(limits.memoryMiB, "limits.memoryMiB", 64);
  integer(limits.pids, "limits.pids", 16);
  integer(limits.dailyTokens, "limits.dailyTokens", 1);
  if (typeof limits.cpus !== "number" || !(limits.cpus > 0)) {
    throw new Error("configuration key limits.cpus must be a positive number");
  }
  return config as Config;
}

function integer(value: unknown, key: string, min: number, max = Number.MAX_SAFE_INTEGER): void {
  if (!Number.isInteger(value) || (value as number) < min || (value as number) > max) {
    throw new Error(`configuration key ${key} must be an integer from ${min} to ${max}`);
  }
}

export function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
