import type { Database } from "bun:sqlite";
import { randomUUID } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import type { LoginRequest, Role, SessionResponse, SetupRequest, SetupResponse, User } from "./api";
import type { Config } from "./config";
import {
  newTotpSecret,
  otpauthUri,
  randomToken,
  type Secrets,
  safeEqual,
  sha256,
  totpStep,
} from "./crypto";
import { HttpError } from "./http";

// OWASP's Argon2id baseline (19 MiB, two passes). Hashes run one at a time,
// so concurrent login attempts cannot multiply the memory they take.
const argon2 = { algorithm: "argon2id", memoryCost: 19456, timeCost: 2 } as const;
let hashing: Promise<unknown> = Promise.resolve();
function serialized<T>(task: () => Promise<T>): Promise<T> {
  const result = hashing.then(task);
  hashing = result.catch(() => {});
  return result;
}

const usernamePattern = /^[a-z0-9][a-z0-9._-]{0,31}$/;
const minPasswordLength = 12;
const tooMany = () =>
  new HttpError(429, "rate_limited", "too many failed attempts; try again later");

/** Counts failures per key in a sliding window; a key at the limit is refused until they age out. */
class RateLimiter {
  private readonly failures = new Map<string, number[]>();

  constructor(
    private readonly limit: number,
    private readonly windowMs: number,
    private readonly now: () => number,
  ) {}

  blocked(key: string): boolean {
    return this.recent(key).length >= this.limit;
  }

  fail(key: string): void {
    if (this.failures.size > 10_000) for (const stale of this.failures.keys()) this.recent(stale);
    this.failures.set(key, [...this.recent(key), this.now()]);
  }

  reset(key: string): void {
    this.failures.delete(key);
  }

  private recent(key: string): number[] {
    const since = this.now() - this.windowMs;
    const recent = (this.failures.get(key) ?? []).filter((time) => time > since);
    if (recent.length === 0) this.failures.delete(key);
    return recent;
  }
}

interface UserRow {
  id: string;
  username: string;
  password_hash: string;
  totp_secret: string;
  totp_last_step: number;
  role: Role;
  created_at: number;
}

type SessionRow = Pick<UserRow, "id" | "username" | "role" | "created_at"> & {
  token_hash: string;
  csrf_token: string;
  expires_at: number;
  last_seen_at: number;
};

export interface Session {
  tokenHash: string;
  csrfToken: string;
  user: User;
}

const toUser = (row: Pick<UserRow, "id" | "username" | "role" | "created_at">): User => ({
  id: row.id,
  username: row.username,
  role: row.role,
  createdAt: new Date(row.created_at).toISOString(),
});

/** Users, first-run setup, login and sessions. */
export class Auth {
  readonly sessionSeconds: number;
  private readonly idleMs: number;
  private readonly limiter: RateLimiter;
  private dummyHash: Promise<string> | undefined;

  constructor(
    private readonly db: Database,
    private readonly secrets: Secrets,
    private readonly config: Pick<
      Config,
      "setupTokenFile" | "sessionIdleMinutes" | "sessionAbsoluteHours"
    >,
    private readonly now: () => number = Date.now,
  ) {
    this.sessionSeconds = config.sessionAbsoluteHours * 3600;
    this.idleMs = config.sessionIdleMinutes * 60_000;
    this.limiter = new RateLimiter(5, 15 * 60_000, now);
  }

  setupNeeded(): boolean {
    return this.db.query("SELECT 1 FROM users LIMIT 1").get() === null;
  }

  /** Creates the first admin, once, with the configured setup token. */
  async setup(request: SetupRequest): Promise<SetupResponse> {
    if (!this.setupNeeded()) {
      throw new HttpError(409, "setup_complete", "setup is already complete");
    }
    if (this.limiter.blocked("setup")) throw tooMany();
    const expected = this.setupToken();
    if (expected === "") {
      throw new HttpError(403, "setup_unavailable", "no setup token is configured");
    }
    if (!safeEqual(request.token, expected)) {
      this.limiter.fail("setup");
      throw new HttpError(403, "invalid_setup_token", "the setup token is invalid");
    }
    const admin = await this.createUser(request.username, request.password, "admin", {
      onlyFirst: true,
    });
    // The token is spent: emptied, the host stops showing it and creates no new one.
    try {
      writeFileSync(this.config.setupTokenFile, "");
    } catch (error) {
      console.error(`cannot empty the setup token file: ${error}`);
    }
    return { user: admin.user, otpauthUri: otpauthUri(admin.user.username, admin.totpSecret) };
  }

  /** Creates a user with a new TOTP secret; with `onlyFirst`, only while no user exists. */
  async createUser(
    username: string,
    password: string,
    role: Role,
    { onlyFirst = false } = {},
  ): Promise<{ user: User; totpSecret: Buffer }> {
    const name = username.toLowerCase();
    if (!usernamePattern.test(name)) {
      throw new HttpError(
        400,
        "invalid_request",
        "username must be 1-32 lowercase letters, digits, '.', '_' or '-'",
      );
    }
    if (password.length < minPasswordLength) {
      throw new HttpError(
        400,
        "invalid_request",
        `password must have at least ${minPasswordLength} characters`,
      );
    }
    const passwordHash = await serialized(() => Bun.password.hash(password, argon2));
    const totpSecret = newTotpSecret();
    const row = { id: randomUUID(), username: name, role, created_at: this.now() };
    // Re-checked after the asynchronous hash, atomically with the insert.
    this.db.transaction(() => {
      if (onlyFirst && !this.setupNeeded()) {
        throw new HttpError(409, "setup_complete", "setup is already complete");
      }
      if (this.db.query("SELECT 1 FROM users WHERE username = ?").get(name) !== null) {
        throw new HttpError(409, "conflict", "the username is taken");
      }
      this.db
        .query(
          "INSERT INTO users (id, username, password_hash, totp_secret, role, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .run(
          row.id,
          name,
          passwordHash,
          this.secrets.encrypt(totpSecret.toString("base64url"), `users.totp_secret:${row.id}`),
          role,
          row.created_at,
        );
    })();
    return { user: toUser(row), totpSecret };
  }

  /** Checks password and TOTP code and opens a session; returns its cookie token. */
  async login(request: LoginRequest): Promise<{ token: string; session: SessionResponse }> {
    const username = request.username.toLowerCase();
    const key = `login:${username}`;
    if (this.limiter.blocked(key)) throw tooMany();
    const row = this.db
      .query("SELECT * FROM users WHERE username = ?")
      .get(username) as UserRow | null;
    // Unknown users cost the same hash verification as known ones.
    this.dummyHash ??= Bun.password.hash(randomToken(), argon2);
    const hash = row?.password_hash ?? (await this.dummyHash);
    const passwordOk = await serialized(() => Bun.password.verify(request.password, hash));
    const now = this.now();
    const step =
      row !== null && passwordOk ? totpStep(this.totpSecret(row), request.code, now) : null;
    // A code is accepted once: only a step newer than the last accepted one
    // updates the row, also when two logins race with the same code.
    if (
      row === null ||
      step === null ||
      this.db
        .query("UPDATE users SET totp_last_step = ? WHERE id = ? AND totp_last_step < ?")
        .run(step, row.id, step).changes === 0
    ) {
      this.limiter.fail(key);
      throw new HttpError(401, "invalid_credentials", "invalid username, password or code");
    }
    this.limiter.reset(key);

    const token = randomToken();
    const csrfToken = randomToken();
    this.db
      .query("DELETE FROM sessions WHERE expires_at <= ? OR last_seen_at <= ?")
      .run(now, now - this.idleMs);
    this.db
      .query(
        "INSERT INTO sessions (token_hash, user_id, csrf_token, expires_at, last_seen_at) VALUES (?, ?, ?, ?, ?)",
      )
      .run(sha256(token), row.id, csrfToken, now + this.sessionSeconds * 1000, now);
    return { token, session: { user: toUser(row), csrfToken } };
  }

  /** The live session for a cookie token; expired sessions are removed. */
  authenticate(token: string | undefined): Session | null {
    if (!token) return null;
    const row = this.db
      .query(
        `SELECT s.token_hash, s.csrf_token, s.expires_at, s.last_seen_at, u.id, u.username, u.role, u.created_at
         FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?`,
      )
      .get(sha256(token)) as SessionRow | null;
    if (row === null) return null;
    const now = this.now();
    if (row.expires_at <= now || row.last_seen_at + this.idleMs <= now) {
      this.logout(row.token_hash);
      return null;
    }
    this.db
      .query("UPDATE sessions SET last_seen_at = ? WHERE token_hash = ?")
      .run(now, row.token_hash);
    return { tokenHash: row.token_hash, csrfToken: row.csrf_token, user: toUser(row) };
  }

  logout(tokenHash: string): void {
    this.db.query("DELETE FROM sessions WHERE token_hash = ?").run(tokenHash);
  }

  private setupToken(): string {
    try {
      return readFileSync(this.config.setupTokenFile, "utf8").trim();
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return "";
      throw error;
    }
  }

  private totpSecret(row: UserRow): Buffer {
    const encoded = this.secrets.decrypt(row.totp_secret, `users.totp_secret:${row.id}`);
    return Buffer.from(encoded, "base64url");
  }
}
