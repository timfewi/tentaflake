import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  randomBytes,
  timingSafeEqual,
} from "node:crypto";
import { readFileSync, statSync } from "node:fs";

export const randomToken = (bytes = 32): string => randomBytes(bytes).toString("base64url");

export const sha256 = (value: string): string => createHash("sha256").update(value).digest("hex");

/** Constant-time comparison; hashing first gives both sides the same length. */
export function safeEqual(a: string, b: string): boolean {
  const digest = (value: string) => createHash("sha256").update(value).digest();
  return timingSafeEqual(digest(a), digest(b));
}

/**
 * AES-256-GCM with the host key. The context (column and row id) is
 * authenticated, so a ciphertext copied to another row does not decrypt.
 */
export class Secrets {
  constructor(private readonly key: Buffer) {
    if (key.length !== 32) throw new Error("the host key must be exactly 32 bytes");
  }

  static fromFile(path: string): Secrets {
    if ((statSync(path).mode & 0o077) !== 0) {
      throw new Error(`the host key file ${path} must not be accessible to group or others`);
    }
    return new Secrets(readFileSync(path));
  }

  encrypt(plaintext: string, context: string): string {
    const iv = randomBytes(12);
    const cipher = createCipheriv("aes-256-gcm", this.key, iv);
    cipher.setAAD(Buffer.from(context));
    const body = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
    return `v1.${Buffer.concat([iv, cipher.getAuthTag(), body]).toString("base64url")}`;
  }

  decrypt(value: string, context: string): string {
    const [version, payload] = value.split(".");
    if (version !== "v1" || payload === undefined) throw new Error("unsupported secret format");
    const raw = Buffer.from(payload, "base64url");
    const decipher = createDecipheriv("aes-256-gcm", this.key, raw.subarray(0, 12));
    decipher.setAAD(Buffer.from(context));
    decipher.setAuthTag(raw.subarray(12, 28));
    return Buffer.concat([decipher.update(raw.subarray(28)), decipher.final()]).toString("utf8");
  }
}

// TOTP (RFC 6238): HMAC-SHA1, 30-second steps, six digits.
const period = 30;

export const newTotpSecret = (): Buffer => randomBytes(20);

export function totpCode(secret: Buffer, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const mac = createHmac("sha1", secret).update(counter).digest();
  const offset = (mac.at(-1) ?? 0) & 0xf;
  return String((mac.readUInt32BE(offset) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

/** The time step that `code` belongs to, allowing one step of clock drift, or null. */
export function totpStep(secret: Buffer, code: string, now: number): number | null {
  if (!/^\d{6}$/.test(code)) return null;
  const current = Math.floor(now / 1000 / period);
  for (const step of [current - 1, current, current + 1]) {
    if (safeEqual(totpCode(secret, step), code)) return step;
  }
  return null;
}

export function otpauthUri(username: string, secret: Buffer): string {
  const label = encodeURIComponent(`Tentaflake:${username}`);
  return `otpauth://totp/${label}?secret=${base32(secret)}&issuer=Tentaflake&algorithm=SHA1&digits=6&period=${period}`;
}

function base32(bytes: Buffer): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let output = "";
  let value = 0;
  let bits = 0;
  for (const byte of bytes) {
    value = ((value << 8) | byte) & 0xfff;
    bits += 8;
    for (; bits >= 5; bits -= 5) output += alphabet[(value >>> (bits - 5)) & 31];
  }
  return bits > 0 ? output + alphabet[(value << (5 - bits)) & 31] : output;
}
