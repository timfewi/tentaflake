import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { uiServer } from "../src/ui";
import { harness, origin } from "./helpers";

/** A built UI in a temporary directory, next to a secret that lies outside it. */
function build() {
  const parent = mkdtempSync(join(tmpdir(), "tentaflake-ui-"));
  const dist = join(parent, "dist");
  mkdirSync(join(dist, "assets"), { recursive: true });
  writeFileSync(join(dist, "index.html"), "<!doctype html><title>ui</title>");
  writeFileSync(join(dist, "assets", "app.js"), "console.log(1)");
  writeFileSync(join(parent, "secret.txt"), "secret");
  return { dist, remove: () => rmSync(parent, { recursive: true, force: true }) };
}

const get = (serve: (request: Request) => Response, path: string, method = "GET") =>
  serve(new Request(`${origin}${path}`, { method }));

describe("UI server", () => {
  test("serves the bundle with security headers and nothing else", async () => {
    const { dist, remove } = build();
    try {
      const serve = uiServer(dist, origin);
      const page = get(serve, "/");
      expect(page.status).toBe(200);
      expect(page.headers.get("content-type")).toContain("text/html");
      expect(await page.text()).toContain("<title>ui</title>");
      const policy = page.headers.get("content-security-policy") ?? "";
      expect(policy).toContain("script-src 'self';");
      expect(policy).toContain("frame-ancestors 'none'");
      expect(policy).toContain("connect-src 'self' wss://tentaflake.example");
      expect(policy).not.toMatch(/script-src[^;]*unsafe/);
      expect(page.headers.get("x-content-type-options")).toBe("nosniff");
      expect(page.headers.get("referrer-policy")).toBe("no-referrer");

      const script = get(serve, "/assets/app.js");
      expect(script.headers.get("content-type")).toContain("javascript");
      expect(script.headers.get("x-content-type-options")).toBe("nosniff");
      expect(await get(serve, "/index.html", "HEAD").text()).toBe("");

      for (const path of ["/secret.txt", "/../secret.txt", "/%2e%2e/secret.txt", "/assets", "/x"]) {
        expect(get(serve, path).status).toBe(404);
      }
      const post = get(serve, "/", "POST");
      expect(post.status).toBe(405);
      expect(post.headers.get("content-security-policy")).toBe(policy);
    } finally {
      remove();
    }
  });

  test("refuses a directory without a build", () => {
    const parent = mkdtempSync(join(tmpdir(), "tentaflake-ui-"));
    try {
      expect(() => uiServer(parent, origin)).toThrow("build the UI first");
    } finally {
      rmSync(parent, { recursive: true, force: true });
    }
  });

  test("the service serves it beside the API, which keeps its own errors", async () => {
    const { dist, remove } = build();
    try {
      const h = harness({ ui: dist });
      expect(await (await fetch(`${h.url}/`)).text()).toContain("<title>ui</title>");
      const missing = await fetch(`${h.url}/api/v1/nothing`);
      expect(missing.status).toBe(404);
      expect(await missing.json()).toMatchObject({ error: "not_found" });
      const api = await fetch(`${h.url}/api/v1/setup`);
      expect((await api.json()) as unknown).toEqual({ needed: true });
    } finally {
      remove();
    }
  });

  test("without a UI directory the service answers the API only", async () => {
    const h = harness();
    expect((await fetch(`${h.url}/`)).status).toBe(404);
  });
});
