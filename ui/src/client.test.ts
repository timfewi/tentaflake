import { afterEach, expect, mock, test } from "bun:test";
import { ApiFailure, api, whenSignedOut } from "./client";

interface Call {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
  whenSignedOut(() => {});
});

/** Replaces fetch with answers in order and records what was sent. */
function answers(...responses: (Response | Error)[]) {
  const calls: Call[] = [];
  globalThis.fetch = mock(async (url: string, init: RequestInit) => {
    calls.push({
      url,
      method: init.method ?? "GET",
      headers: (init.headers ?? {}) as Record<string, string>,
      body: init.body === undefined ? undefined : JSON.parse(String(init.body)),
    });
    const next = responses.shift();
    if (next === undefined || next instanceof Error) throw next ?? new Error("no answer left");
    return next;
  }) as unknown as typeof fetch;
  return calls;
}

const session = {
  user: { id: "u", username: "admin", role: "admin", createdAt: "" },
  csrfToken: "csrf-1",
};
const failure = (status: number, error: string, message = "no") =>
  Response.json({ error, message }, { status });

test("mutations carry the CSRF token of the session and JSON bodies", async () => {
  const calls = answers(
    Response.json(session),
    Response.json({ keys: [] }),
    Response.json(
      { id: "k", provider: "openai", label: "x", baseUrl: null, createdAt: "" },
      { status: 201 },
    ),
    new Response(null, { status: 204 }),
  );
  await api.login({ username: "admin", password: "p", code: "123456" });
  await api.keys();
  await api.addKey({ provider: "openai", label: "x", secret: "s" });
  expect(await api.deleteKey("k")).toBeUndefined();

  expect(calls.map((call) => [call.method, call.url])).toEqual([
    ["POST", "/api/v1/session"],
    ["GET", "/api/v1/keys"],
    ["POST", "/api/v1/keys"],
    ["DELETE", "/api/v1/keys/k"],
  ]);
  expect(calls[1]?.headers["x-csrf-token"]).toBeUndefined();
  expect(calls[2]?.headers).toEqual({
    "content-type": "application/json",
    "x-csrf-token": "csrf-1",
  });
  expect(calls[2]?.body).toEqual({ provider: "openai", label: "x", secret: "s" });
  expect(calls[3]?.headers["x-csrf-token"]).toBe("csrf-1");
});

test("an API error keeps its status, code and message", async () => {
  answers(failure(409, "conflict", "the environment is not running"));
  const error = await api.sessions("e1").catch((caught) => caught);
  expect(error).toBeInstanceOf(ApiFailure);
  expect(error).toMatchObject({
    status: 409,
    code: "conflict",
    message: "the environment is not running",
  });
});

test("an answer that is not the API's error envelope is still an error", async () => {
  answers(new Response("bad gateway", { status: 502 }));
  expect(await api.environments().catch((caught) => caught)).toMatchObject({
    status: 502,
    code: "internal",
    message: "the service answered 502",
  });
});

test("an unreachable service is a network failure", async () => {
  answers(new TypeError("fetch failed"));
  expect(await api.keys().catch((caught) => caught)).toMatchObject({ status: 0, code: "network" });
});

test("only a lost session signs the user out, not a wrong password", async () => {
  const signedOut = mock(() => {});
  whenSignedOut(signedOut);
  answers(failure(401, "invalid_credentials"), failure(401, "unauthenticated"));
  await api.login({ username: "a", password: "b", code: "c" }).catch(() => {});
  expect(signedOut).not.toHaveBeenCalled();
  await api.keys().catch(() => {});
  expect(signedOut).toHaveBeenCalledTimes(1);
});

test("paths and names are encoded into the URL, file bytes come back raw", async () => {
  const calls = answers(
    Response.json({ entries: [{ path: "a b/c.txt", type: "file" }] }),
    new Response(new Uint8Array([104, 105, 0, 255])),
    Response.json({
      server: { name: "docs", type: "remote", url: "https://x.example", secrets: [] },
    }),
  );
  expect(await api.files("e1", "a b/c d")).toEqual([{ path: "a b/c.txt", type: "file" }]);
  expect([...(await api.fileContent("e1", "a&b=c.bin"))]).toEqual([104, 105, 0, 255]);
  await api.putMcpServer("e1", "do/cs", { type: "remote", url: "https://x.example" });
  expect(calls.map((call) => call.url)).toEqual([
    "/api/v1/environments/e1/files?path=a%20b%2Fc%20d",
    "/api/v1/environments/e1/files/content?path=a%26b%3Dc.bin",
    "/api/v1/environments/e1/mcp/do%2Fcs",
  ]);
  expect(calls[2]?.method).toBe("PUT");
});
