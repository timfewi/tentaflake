import type { Database } from "bun:sqlite";
import { randomUUID } from "node:crypto";
import type { CreateKeyRequest, ModelKey, Provider } from "./api";
import type { BrokerFiles } from "./broker";
import type { Secrets } from "./crypto";
import { HttpError } from "./http";

/**
 * Per provider: the API family the broker speaks, the upstream base URL and
 * the AI SDK package OpenCode uses (checked against OpenCode 2.0.29, which
 * bundles these three packages).
 */
export const providers: Record<
  Provider,
  { api: "openai" | "anthropic"; baseUrl: string | null; package: string }
> = {
  anthropic: {
    api: "anthropic",
    baseUrl: "https://api.anthropic.com/v1/",
    package: "aisdk:@ai-sdk/anthropic",
  },
  openai: { api: "openai", baseUrl: "https://api.openai.com/v1/", package: "aisdk:@ai-sdk/openai" },
  openrouter: {
    api: "openai",
    baseUrl: "https://openrouter.ai/api/v1/",
    package: "aisdk:@ai-sdk/openai-compatible",
  },
  "openai-compatible": { api: "openai", baseUrl: null, package: "aisdk:@ai-sdk/openai-compatible" },
};

export const keySecretContext = (id: string) => `model_keys.secret:${id}`;

interface KeyRow {
  id: string;
  provider: Provider;
  label: string;
  base_url: string | null;
  created_at: number;
}

const toKey = (row: KeyRow): ModelKey => ({
  id: row.id,
  provider: row.provider,
  label: row.label,
  baseUrl: row.base_url,
  createdAt: new Date(row.created_at).toISOString(),
});

const invalid = (message: string) => new HttpError(400, "invalid_request", message);

/** Model API keys per owner. Secrets are encrypted at rest and never returned. */
export class Keys {
  constructor(
    private readonly db: Database,
    private readonly secrets: Secrets,
    private readonly broker: BrokerFiles,
    private readonly now: () => number = Date.now,
  ) {}

  list(ownerId: string): ModelKey[] {
    const rows = this.db
      .query(
        "SELECT id, provider, label, base_url, created_at FROM model_keys WHERE owner_id = ? ORDER BY created_at, id",
      )
      .all(ownerId) as KeyRow[];
    return rows.map(toKey);
  }

  add(ownerId: string, request: CreateKeyRequest): ModelKey {
    const { provider, label } = request;
    if (!Object.hasOwn(providers, provider)) {
      throw invalid(`provider must be one of ${Object.keys(providers).join(", ")}`);
    }
    if (label.trim() === "" || label.length > 64) {
      throw invalid("label must be 1-64 characters");
    }
    // The broker reads the key as one line of at most 4096 bytes.
    const secret = request.secret.trim();
    if (secret === "" || Buffer.byteLength(secret) > 4096 || /\p{Cc}/u.test(secret)) {
      throw invalid("secret must be one line of at most 4096 bytes");
    }
    if ((provider === "openai-compatible") !== (request.baseUrl !== undefined)) {
      throw invalid("baseUrl is required for openai-compatible keys and only for them");
    }
    const row: KeyRow = {
      id: randomUUID(),
      provider,
      label,
      base_url: request.baseUrl === undefined ? null : baseUrl(request.baseUrl),
      created_at: this.now(),
    };
    this.db
      .query(
        "INSERT INTO model_keys (id, owner_id, provider, label, base_url, secret, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      )
      .run(
        row.id,
        ownerId,
        provider,
        label,
        row.base_url,
        this.secrets.encrypt(secret, keySecretContext(row.id)),
        row.created_at,
      );
    return toKey(row);
  }

  /** Environments using the key lose model access: their routes and credential files go. */
  delete(ownerId: string, id: string): void {
    if (
      this.db.query("DELETE FROM model_keys WHERE id = ? AND owner_id = ?").run(id, ownerId)
        .changes === 0
    ) {
      throw new HttpError(404, "not_found", "no such key");
    }
    this.broker.sync();
  }
}

// The broker requires HTTPS on a public address; that is checked per request.
function baseUrl(value: string): string {
  const url = URL.canParse(value) ? new URL(value) : null;
  if (
    url === null ||
    !["http:", "https:"].includes(url.protocol) ||
    url.username !== "" ||
    url.password !== "" ||
    url.search !== "" ||
    url.hash !== "" ||
    value.length > 2048
  ) {
    throw invalid("baseUrl must be an http(s) URL without credentials, query or fragment");
  }
  return url.href.endsWith("/") ? url.href : `${url.href}/`;
}
