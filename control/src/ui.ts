import { readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const textResponse = (status: number, text: string, headers: Record<string, string> = {}) =>
  new Response(`${text}\n`, { status, headers });

/**
 * Serves the built UI (ui/dist) from memory-listed files: only paths that exist
 * when the service starts are ever served. Scripts come from the page's own
 * origin and never inline; styles may be inline because the terminal emulator
 * sets them.
 */
export function uiServer(directory: string, origin: () => string): (request: Request) => Response {
  const root = resolve(directory);
  const files = new Map<string, string>();
  for (const entry of readdirSync(root, { recursive: true, withFileTypes: true })) {
    if (entry.isFile()) {
      const path = join(entry.parentPath, entry.name);
      files.set(`/${relative(root, path).split("\\").join("/")}`, path);
    }
  }
  if (!files.has("/index.html")) throw new Error(`${root} has no index.html; build the UI first`);

  const policy = () =>
    [
      "default-src 'none'",
      "script-src 'self'",
      "style-src 'self' 'unsafe-inline'",
      "img-src 'self'",
      `connect-src 'self' ${origin().replace(/^http/, "ws")}`,
      "base-uri 'none'",
      "form-action 'none'",
      "frame-ancestors 'none'",
    ].join("; ");
  const security = {
    "x-content-type-options": "nosniff",
    "x-frame-options": "DENY",
    "referrer-policy": "no-referrer",
    "cross-origin-opener-policy": "same-origin",
    "cross-origin-resource-policy": "same-origin",
    "cache-control": "no-cache",
  };

  return (request) => {
    const headers = { "content-security-policy": policy(), ...security };
    if (request.method !== "GET" && request.method !== "HEAD") {
      return textResponse(405, "method not allowed", { ...headers, allow: "GET, HEAD" });
    }
    const { pathname } = new URL(request.url);
    const file = files.get(pathname === "/" ? "/index.html" : pathname);
    if (file === undefined) return textResponse(404, "not found", headers);
    return new Response(request.method === "HEAD" ? null : Bun.file(file), {
      headers: { ...headers, "content-type": Bun.file(file).type },
    });
  };
}
