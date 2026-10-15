import { describe, expect, test } from "bun:test";
import type { SessionResponse, SetupResponse } from "../src/api";
import {
  base32Decode,
  call,
  type Harness,
  harness,
  newUser,
  nextCode,
  origin,
  password,
  setupToken,
  signIn,
} from "./helpers";

const admin = { username: "admin", password };

async function setUp(h: Harness): Promise<Buffer> {
  const response = await call(h, "POST", "/api/v1/setup", { token: setupToken, ...admin });
  expect(response.status).toBe(201);
  const { user, otpauthUri } = (await response.json()) as SetupResponse;
  expect(user).toMatchObject({ username: "admin", role: "admin" });
  const uri = new URL(otpauthUri);
  expect(uri.protocol).toBe("otpauth:");
  return base32Decode(uri.searchParams.get("secret") ?? "");
}

describe("setup", () => {
  test("creates the admin once, and only with the setup token", async () => {
    const h = harness();
    expect(await (await call(h, "GET", "/api/v1/setup")).json()).toEqual({ needed: true });

    const missing = await call(h, "POST", "/api/v1/setup", admin);
    expect(missing.status).toBe(400);
    const wrong = await call(h, "POST", "/api/v1/setup", { token: "wrong", ...admin });
    expect(wrong.status).toBe(403);
    expect(((await wrong.json()) as { error: string }).error).toBe("invalid_setup_token");
    expect(h.auth.setupNeeded()).toBe(true);

    const secret = await setUp(h);
    expect(secret.length).toBe(20);
    expect(await (await call(h, "GET", "/api/v1/setup")).json()).toEqual({ needed: false });

    const again = await call(h, "POST", "/api/v1/setup", { token: setupToken, ...admin });
    expect(again.status).toBe(409);
    const other = { token: setupToken, username: "second", password };
    expect((await call(h, "POST", "/api/v1/setup", other)).status).toBe(409);
    expect(h.db.query("SELECT COUNT(*) AS n FROM users").get()).toEqual({ n: 1 });
  });

  test("concurrent setup requests create one admin", async () => {
    const h = harness();
    const body = { token: setupToken, ...admin };
    const [first, second] = await Promise.all([
      call(h, "POST", "/api/v1/setup", body),
      call(h, "POST", "/api/v1/setup", { ...body, username: "other" }),
    ]);
    expect([first?.status, second?.status].sort()).toEqual([201, 409]);
    expect(h.db.query("SELECT COUNT(*) AS n FROM users").get()).toEqual({ n: 1 });
  });

  test("is unavailable without a setup token file", async () => {
    const h = harness({ token: null });
    const response = await call(h, "POST", "/api/v1/setup", { token: "", ...admin });
    expect(response.status).toBe(403);
    expect(((await response.json()) as { error: string }).error).toBe("setup_unavailable");
  });

  test("stores the TOTP secret encrypted and the password as Argon2id", async () => {
    const h = harness();
    const secret = await setUp(h);
    const row = h.db.query("SELECT * FROM users").get() as Record<string, string>;
    expect(row.password_hash).toStartWith("$argon2id$");
    expect(row.totp_secret).toStartWith("v1.");
    expect(row.totp_secret).not.toContain(secret.toString("base64url"));
  });

  test("refuses a weak password and an invalid username", async () => {
    const h = harness();
    const short = await call(h, "POST", "/api/v1/setup", {
      ...admin,
      token: setupToken,
      password: "short",
    });
    expect(short.status).toBe(400);
    const name = await call(h, "POST", "/api/v1/setup", {
      ...admin,
      token: setupToken,
      username: "a b",
    });
    expect(name.status).toBe(400);
    expect(h.auth.setupNeeded()).toBe(true);
  });
});

describe("login", () => {
  test("refuses a wrong password, a wrong code and a replayed code", async () => {
    const h = harness();
    const secret = await setUp(h);
    const login = (body: object) => call(h, "POST", "/api/v1/session", body);

    const code = nextCode(h, secret);
    const wrongPassword = await login({ username: "admin", password: "wrong password!", code });
    expect(wrongPassword.status).toBe(401);
    expect(wrongPassword.headers.get("set-cookie")).toBeNull();
    const wrongCode = String((Number(code) + 1) % 1_000_000).padStart(6, "0");
    expect((await login({ ...admin, code: wrongCode })).status).toBe(401);
    expect((await login({ username: "nobody", password, code })).status).toBe(401);

    const ok = await login({ ...admin, code });
    expect(ok.status).toBe(200);
    const cookie = ok.headers.get("set-cookie") ?? "";
    expect(cookie).toMatch(/^__Host-tentaflake=[\w-]{43}; Path=\/; Max-Age=43200;/);
    for (const attribute of ["Secure", "HttpOnly", "SameSite=Strict"]) {
      expect(cookie.split("; ")).toContain(attribute);
    }
    expect((await login({ ...admin, code })).status).toBe(401);

    const next = nextCode(h, secret);
    const racing = await Promise.all([
      login({ ...admin, code: next }),
      login({ ...admin, code: next }),
    ]);
    expect(racing.map((response) => response.status).sort()).toEqual([200, 401]);
  });

  test("is rate-limited per username", async () => {
    const h = harness();
    const secret = await setUp(h);
    for (let attempt = 0; attempt < 5; attempt++) {
      const body = { username: "admin", password: "wrong password!", code: "000000" };
      expect((await call(h, "POST", "/api/v1/session", body)).status).toBe(401);
    }
    const code = nextCode(h, secret);
    const limited = await call(h, "POST", "/api/v1/session", { ...admin, code });
    expect(limited.status).toBe(429);

    h.clock.now += 15 * 60_000;
    await signIn(h, "admin", secret);
  });

  test("the session expires when idle and at its absolute limit, and on logout", async () => {
    const h = harness();
    const secret = await setUp(h);
    const session = (client: object) => call(h, "GET", "/api/v1/session", undefined, client);

    let client = await signIn(h, "admin", secret);
    const current = (await (await session(client)).json()) as SessionResponse;
    expect(current.user.username).toBe("admin");
    expect(current.csrfToken).toBe(client.csrf ?? "");
    h.clock.now += 61 * 60_000;
    expect((await session(client)).status).toBe(401);

    // Active every 59 minutes, it still ends 12 hours after the login.
    client = await signIn(h, "admin", secret);
    for (let hour = 0; hour < 12; hour++) {
      h.clock.now += 59 * 60_000;
      expect((await session(client)).status).toBe(200);
    }
    h.clock.now += 12 * 60_000;
    expect((await session(client)).status).toBe(401);

    client = await signIn(h, "admin", secret);
    const logout = await call(h, "DELETE", "/api/v1/session", undefined, client);
    expect(logout.status).toBe(204);
    expect(logout.headers.get("set-cookie")).toStartWith("__Host-tentaflake=; Path=/; Max-Age=0;");
    expect((await session(client)).status).toBe(401);
  });
});

describe("mutations", () => {
  test("need the session's CSRF token", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const create = (csrf?: string) =>
      call(h, "POST", "/api/v1/environments", { name: "dev" }, { ...client, csrf });

    for (const csrf of [undefined, "", "wrong"]) {
      const response = await create(csrf);
      expect(response.status).toBe(403);
      expect(((await response.json()) as { error: string }).error).toBe("csrf_mismatch");
    }
    const other = await newUser(h, "other");
    expect((await create(other.client.csrf)).status).toBe(403);
    expect(h.docker.containers.size).toBe(0);

    expect((await create(client.csrf)).status).toBe(201);
    const logout = await call(h, "DELETE", "/api/v1/session", undefined, { ...client, csrf: "x" });
    expect(logout.status).toBe(403);
  });

  test("need the exact configured Origin", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    for (const value of [
      null,
      "null",
      "https://evil.example",
      `${origin}/`,
      "http://tentaflake.example",
    ]) {
      const response = await call(h, "POST", "/api/v1/environments", { name: "dev" }, client, {
        origin: value,
      });
      expect(response.status).toBe(403);
      expect(((await response.json()) as { error: string }).error).toBe("origin_mismatch");
    }
    const setup = await call(
      h,
      "POST",
      "/api/v1/setup",
      { token: setupToken, ...admin },
      {},
      {
        origin: "https://evil.example",
      },
    );
    expect(setup.status).toBe(403);
    expect(h.docker.containers.size).toBe(0);
  });

  test("without a session are refused", async () => {
    const h = harness();
    const response = await call(
      h,
      "POST",
      "/api/v1/environments",
      { name: "dev" },
      {
        cookie: "forged",
        csrf: "forged",
      },
    );
    expect(response.status).toBe(401);
    expect((await call(h, "GET", "/api/v1/environments")).status).toBe(401);
  });

  test("need a JSON body", async () => {
    const h = harness();
    const response = await fetch(`${h.url}/api/v1/setup`, {
      method: "POST",
      headers: { origin, "content-type": "application/x-www-form-urlencoded" },
      body: "token=x",
    });
    expect(response.status).toBe(415);
    expect((await call(h, "GET", "/api/v1/nothing")).status).toBe(404);
  });
});
