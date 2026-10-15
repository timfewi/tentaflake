import type { Database } from "bun:sqlite";
import { mkdirSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Provider } from "./api";
import { randomToken, type Secrets, sha256 } from "./crypto";
import { keySecretContext, providers } from "./keys";

export const brokerTokenContext = (id: string) => `environments.broker_token:${id}`;

interface RouteRow {
  id: string;
  broker_token: string;
  models: string;
  limits: string;
  key_id: string;
  provider: Provider;
  base_url: string | null;
  secret: string;
}

/**
 * The files the broker reads (crates/tentaflake-broker), kept in step with the
 * database: a provider credential file per environment with a key, and
 * `routes.json`, which maps each environment's token hash to its upstream,
 * credential file, models and budget. The broker reloads it on its next request.
 */
export class BrokerFiles {
  constructor(
    private readonly db: Database,
    private readonly secrets: Secrets,
    private readonly directory: string,
  ) {}

  /** Rewrites every file: credentials first, then routes; stale credentials go last. */
  sync(): void {
    mkdirSync(this.directory, { recursive: true, mode: 0o700 });
    const rows = this.db
      .query(
        `SELECT e.id, e.broker_token, e.models, e.limits, k.id AS key_id, k.provider, k.base_url, k.secret
         FROM environments e JOIN model_keys k ON k.id = e.key_id ORDER BY e.id`,
      )
      .all() as RouteRow[];
    const routes = rows.map((row) => {
      const credentialFile = join(this.directory, `${row.id}.key`);
      this.write(credentialFile, this.secrets.decrypt(row.secret, keySecretContext(row.key_id)));
      const { api, baseUrl } = providers[row.provider];
      return {
        id: row.id,
        token_sha256: sha256(this.secrets.decrypt(row.broker_token, brokerTokenContext(row.id))),
        api,
        upstream_base_url: row.base_url ?? baseUrl,
        credential_file: credentialFile,
        models: (JSON.parse(row.models) as string[]).map((name) => ({ name })),
        daily_token_budget: (JSON.parse(row.limits) as { dailyTokens: number }).dailyTokens,
      };
    });
    this.write(join(this.directory, "routes.json"), `${JSON.stringify({ routes })}\n`);
    const current = new Set(rows.map((row) => `${row.id}.key`));
    for (const file of readdirSync(this.directory)) {
      if ((file.endsWith(".key") && !current.has(file)) || file.endsWith(".tmp")) {
        rmSync(join(this.directory, file), { force: true });
      }
    }
  }

  /** Replaces a file atomically with a private (0600) one. */
  private write(path: string, content: string): void {
    const temporary = `${path}.${randomToken(9)}.tmp`;
    writeFileSync(temporary, content, { mode: 0o600, flag: "wx" });
    renameSync(temporary, path);
  }
}
