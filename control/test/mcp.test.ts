import { describe, expect, test } from "bun:test";
import type { Environment, McpServerList, McpServerResponse } from "../src/api";
import {
  type Client,
  call,
  createEnvironment,
  envOf,
  type Harness,
  harness,
  newUser,
} from "./helpers";

async function setup(h: Harness = harness()) {
  const { client } = await newUser(h, "member");
  const env = (await (await createEnvironment(h, client)).json()) as Environment;
  return { h, client, env, path: `/api/v1/environments/${env.id}/mcp` };
}

const put = (h: Harness, client: Client, path: string, body: object) =>
  call(h, "PUT", path, body, client);

/** The only container and the `mcp` section of its generated OpenCode configuration. */
function container(h: Harness) {
  expect(h.docker.containers.size).toBe(1);
  const entry = [...h.docker.containers][0];
  if (entry === undefined) throw new Error("no container");
  const [id, { spec, running }] = entry;
  const config = JSON.parse(envOf(spec.Env).OPENCODE_CONFIG_CONTENT ?? "{}");
  return { id, running, mcp: config.mcp as Record<string, unknown> };
}

describe("MCP servers", () => {
  test("go into the configuration of a recreated container; secrets only on request", async () => {
    const { h, client, env, path } = await setup();
    const before = container(h).id;

    const local = await put(h, client, `${path}/files`, {
      type: "local",
      command: ["npx", "-y", "files-mcp"],
      environment: { LOG_LEVEL: "debug" },
      secrets: { FILES_TOKEN: "mcp-secret-value" },
    });
    expect(local.status).toBe(200);
    const response = (await local.json()) as McpServerResponse;
    expect(response.warning).toContain("can read");
    expect(response.server).toEqual({
      name: "files",
      type: "local",
      command: ["npx", "-y", "files-mcp"],
      environment: { LOG_LEVEL: "debug" },
      secrets: ["FILES_TOKEN"],
    });
    const remote = await put(h, client, `${path}/search`, {
      type: "remote",
      url: "https://mcp.example/search",
      headers: { "X-Client": "tentaflake" },
    });
    expect((await remote.json()) as McpServerResponse).not.toHaveProperty("warning");

    const now = container(h);
    expect(now.id).not.toBe(before);
    expect(now.running).toBe(true);
    expect(now.mcp).toEqual({
      files: {
        type: "local",
        command: ["npx", "-y", "files-mcp"],
        environment: { LOG_LEVEL: "debug", FILES_TOKEN: "mcp-secret-value" },
      },
      search: {
        type: "remote",
        url: "https://mcp.example/search",
        headers: { "X-Client": "tentaflake" },
      },
    });
    const listed = await (await call(h, "GET", path, undefined, client)).text();
    expect(listed).not.toContain("mcp-secret-value");
    expect((JSON.parse(listed) as McpServerList).servers.map((s) => [s.name, s.secrets])).toEqual([
      ["files", ["FILES_TOKEN"]],
      ["search", []],
    ]);
    const stored = JSON.stringify(h.db.query("SELECT * FROM mcp_servers").all());
    expect(stored).not.toContain("mcp-secret-value");
    expect(env.state).toBe("running");
  });

  test("replacing a server drops its secrets; deleting it recreates the container", async () => {
    const { h, client, path } = await setup();
    const server = { type: "remote", url: "https://mcp.example/a" };
    await put(h, client, `${path}/a`, { ...server, secrets: { Authorization: "Bearer one" } });
    // A changed URL must never receive the secret assigned for the old one.
    await put(h, client, `${path}/a`, { ...server, url: "https://elsewhere.example/" });
    expect(container(h).mcp).toEqual({
      a: { type: "remote", url: "https://elsewhere.example/", headers: {} },
    });

    const before = container(h).id;
    expect((await call(h, "DELETE", `${path}/a`, undefined, client)).status).toBe(204);
    expect(container(h)).toMatchObject({ mcp: {}, running: true });
    expect(container(h).id).not.toBe(before);
    expect((await call(h, "DELETE", `${path}/a`, undefined, client)).status).toBe(404);
  });

  test("keep a stopped environment stopped", async () => {
    const { h, client, env, path } = await setup();
    await call(h, "POST", `/api/v1/environments/${env.id}/stop`, undefined, client);
    await put(h, client, `${path}/a`, { type: "local", command: ["mcp"] });
    expect(container(h).running).toBe(false);
    const fetched = await call(h, "GET", `/api/v1/environments/${env.id}`, undefined, client);
    expect(((await fetched.json()) as Environment).state).toBe("stopped");
  });

  test("are validated before the container changes", async () => {
    const { h, client, path } = await setup();
    const before = container(h).id;
    const local = { type: "local", command: ["mcp"] };
    for (const [name, body] of [
      ["Bad", local],
      ["a", { type: "stdio", command: ["mcp"] }],
      ["a", { ...local, command: [] }],
      ["a", { ...local, command: "mcp" }],
      ["a", { ...local, url: "https://mcp.example/" }],
      ["a", { ...local, environment: { "BAD-NAME": "x" } }],
      ["a", { ...local, secrets: { TOKEN: 1 } }],
      ["a", { type: "remote", url: "file:///etc/passwd" }],
      ["a", { type: "remote", url: "https://mcp.example/", headers: { "X-A": "a\r\nb" } }],
    ] as const) {
      expect((await put(h, client, `${path}/${name}`, body)).status).toBe(400);
    }
    expect(container(h).id).toBe(before);
  });

  test("of another user's environment are not found", async () => {
    const { h, path } = await setup();
    const other = await newUser(h, "other");
    expect((await call(h, "GET", path, undefined, other.client)).status).toBe(404);
    const body = { type: "local", command: ["mcp"] };
    expect((await put(h, other.client, `${path}/a`, body)).status).toBe(404);
    expect((await call(h, "DELETE", `${path}/a`, undefined, other.client)).status).toBe(404);
  });

  test("fail visibly when Docker fails during the restart", async () => {
    const { h, client, env, path } = await setup();
    const body = { type: "local", command: ["mcp"] };
    const state = async () =>
      (
        (await (
          await call(h, "GET", `/api/v1/environments/${env.id}`, undefined, client)
        ).json()) as Environment
      ).state;
    h.docker.failing.add("removeContainer");
    expect((await put(h, client, `${path}/a`, body)).status).toBe(502);
    expect(await state()).toBe("running");
    h.docker.failing.clear();
    h.docker.failing.add("createContainer");
    expect((await put(h, client, `${path}/a`, body)).status).toBe(502);
    expect(await state()).toBe("failed");
    h.docker.failing.clear();
    expect(
      (await call(h, "DELETE", `/api/v1/environments/${env.id}`, undefined, client)).status,
    ).toBe(204);
  });
});
