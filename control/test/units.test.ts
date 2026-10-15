import { Database } from "bun:sqlite";
import { describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadConfig, parseConfig } from "../src/config";
import { otpauthUri, Secrets, totpCode, totpStep } from "../src/crypto";
import { openDatabase } from "../src/db";

const withDir = (run: (dir: string) => void) => {
  const dir = mkdtempSync(join(tmpdir(), "tentaflake-control-"));
  try {
    run(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

describe("TOTP", () => {
  // RFC 6238, appendix B (SHA-1), truncated to six digits.
  const secret = Buffer.from("12345678901234567890");

  test("matches the RFC 6238 test vectors", () => {
    for (const [seconds, code] of [
      [59, "287082"],
      [1111111109, "081804"],
      [1234567890, "005924"],
      [2000000000, "279037"],
    ] as const) {
      expect(totpCode(secret, Math.floor(seconds / 30))).toBe(code);
    }
  });

  test("accepts one step of drift and nothing else", () => {
    const now = 1111111109_000;
    const step = Math.floor(now / 30_000);
    expect(totpStep(secret, "081804", now)).toBe(step);
    expect(totpStep(secret, totpCode(secret, step + 1), now)).toBe(step + 1);
    expect(totpStep(secret, totpCode(secret, step - 1), now)).toBe(step - 1);
    expect(totpStep(secret, totpCode(secret, step + 2), now)).toBeNull();
    expect(totpStep(secret, "81804", now)).toBeNull();
    expect(totpStep(secret, "08180a", now)).toBeNull();
  });

  test("encodes the secret as base32 in the otpauth URI", () => {
    expect(otpauthUri("admin", secret)).toBe(
      "otpauth://totp/Tentaflake%3Aadmin?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Tentaflake&algorithm=SHA1&digits=6&period=30",
    );
  });
});

describe("Secrets", () => {
  test("decrypt only with the same key and context", () => {
    const secrets = new Secrets(randomBytes(32));
    const sealed = secrets.encrypt("hunter2", "users.totp_secret:1");
    expect(sealed).not.toContain("hunter2");
    expect(secrets.encrypt("hunter2", "users.totp_secret:1")).not.toBe(sealed);
    expect(secrets.decrypt(sealed, "users.totp_secret:1")).toBe("hunter2");
    expect(() => secrets.decrypt(sealed, "users.totp_secret:2")).toThrow();
    expect(() => new Secrets(randomBytes(32)).decrypt(sealed, "users.totp_secret:1")).toThrow();
  });

  test("need a private 32-byte key file", () => {
    expect(() => new Secrets(randomBytes(16))).toThrow("32 bytes");
    withDir((dir) => {
      const keyFile = join(dir, "key");
      writeFileSync(keyFile, randomBytes(32), { mode: 0o644 });
      expect(() => Secrets.fromFile(keyFile)).toThrow("group or others");
      chmodSync(keyFile, 0o600);
      expect(Secrets.fromFile(keyFile)).toBeInstanceOf(Secrets);
    });
  });
});

describe("configuration", () => {
  const required = {
    origin: "https://tentaflake.example",
    databasePath: "/var/lib/tentaflake/control.sqlite",
    keyFile: "/var/lib/tentaflake/control.key",
    setupTokenFile: "/var/lib/tentaflake/setup-token",
    image: "tentaflake-runtime:0.5",
  };

  test("the example parses and the defaults listen on loopback", () => {
    const example = loadConfig(join(import.meta.dir, "..", "config.example.json"));
    expect(example.hostname).toBe("127.0.0.1");
    expect(parseConfig(required)).toMatchObject({
      hostname: "127.0.0.1",
      dockerSocket: "/run/docker.sock",
      limits: { memoryMiB: 2048, cpus: 2, pids: 512 },
    });
    expect(parseConfig({ ...required, limits: { pids: 100 } }).limits).toEqual({
      memoryMiB: 2048,
      cpus: 2,
      pids: 100,
    });
  });

  test("refuses unknown keys, missing keys and invalid values", () => {
    const { image: _, ...withoutImage } = required;
    for (const input of [
      [],
      withoutImage,
      { ...required, typo: true },
      { ...required, origin: "https://tentaflake.example/" },
      { ...required, origin: "tentaflake.example" },
      { ...required, port: 70000 },
      { ...required, limits: { memoryMiB: 1.5 } },
      { ...required, limits: { cpus: 0 } },
      { ...required, limits: { disk: 1 } },
      { ...required, sessionIdleMinutes: 0 },
    ]) {
      expect(() => parseConfig(input)).toThrow();
    }
  });
});

describe("database", () => {
  test("migrates once and refuses a newer schema", () => {
    withDir((dir) => {
      const path = join(dir, "control.sqlite");
      openDatabase(path).close();
      const db = openDatabase(path);
      expect(db.query("PRAGMA user_version").get()).toEqual({ user_version: 1 });
      db.close();

      const newer = new Database(path);
      newer.run("PRAGMA user_version = 99");
      newer.close();
      expect(() => openDatabase(path)).toThrow("newer");
    });
  });
});
