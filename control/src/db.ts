import { Database } from "bun:sqlite";

// Append-only: migration n brings the schema from user_version n to n + 1.
const migrations = [
  `CREATE TABLE users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    totp_secret TEXT NOT NULL,
    totp_last_step INTEGER NOT NULL DEFAULT 0,
    role TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    created_at INTEGER NOT NULL
  ) STRICT;
  CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    csrf_token TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
  ) STRICT;
  CREATE TABLE environments (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES users (id),
    name TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('creating', 'running', 'stopped', 'failed')),
    container_id TEXT,
    workspace_volume TEXT NOT NULL,
    network_mode TEXT NOT NULL CHECK (network_mode IN ('model-only', 'open')),
    limits TEXT NOT NULL,
    opencode_password TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_activity_at INTEGER NOT NULL,
    UNIQUE (owner_id, name)
  ) STRICT;`,
  `CREATE TABLE model_keys (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES users (id),
    provider TEXT NOT NULL
      CHECK (provider IN ('anthropic', 'openai', 'openrouter', 'openai-compatible')),
    label TEXT NOT NULL,
    base_url TEXT,
    secret TEXT NOT NULL,
    created_at INTEGER NOT NULL
  ) STRICT;
  ALTER TABLE environments ADD COLUMN key_id TEXT REFERENCES model_keys (id) ON DELETE SET NULL;
  ALTER TABLE environments ADD COLUMN models TEXT NOT NULL DEFAULT '[]';
  ALTER TABLE environments ADD COLUMN broker_token TEXT;
  UPDATE environments SET limits = json_set(limits, '$.dailyTokens', 10000000);
  CREATE TABLE mcp_servers (
    environment_id TEXT NOT NULL REFERENCES environments (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    definition TEXT NOT NULL,
    secrets TEXT,
    PRIMARY KEY (environment_id, name)
  ) STRICT;`,
];

/** Opens (or creates) the database and brings its schema up to date. */
export function openDatabase(path: string): Database {
  const db = new Database(path, { create: true, strict: true });
  db.run("PRAGMA foreign_keys = ON");
  db.run("PRAGMA journal_mode = WAL");
  const { user_version: version } = db.query("PRAGMA user_version").get() as {
    user_version: number;
  };
  if (version > migrations.length) {
    throw new Error(`the database schema (${version}) is newer than this service`);
  }
  db.transaction(() => {
    for (const migration of migrations.slice(version)) db.run(migration);
    db.run(`PRAGMA user_version = ${migrations.length}`);
  })();
  return db;
}
