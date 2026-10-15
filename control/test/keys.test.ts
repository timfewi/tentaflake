import { describe, expect, spyOn, test } from "bun:test";
import { existsSync, readFileSync, rmSync, statSync } from "node:fs";
import { join } from "node:path";
import type { Environment, KeyList, ModelKey } from "../src/api";
import { BrokerFiles } from "../src/broker";
import { sha256 } from "../src/crypto";
import {
  type Client,
  call,
  createEnvironment,
  envOf,
  type Harness,
  harness,
  newUser,
  providerSecret,
} from "./helpers";

const addKey = (h: Harness, client: Client, body: object) =>
  call(h, "POST", "/api/v1/keys", body, client);

const listKeys = async (h: Harness, client: Client) =>
  ((await (await call(h, "GET", "/api/v1/keys", undefined, client)).json()) as KeyList).keys;

async function environment(h: Harness, client: Client, body: object = {}): Promise<Environment> {
  const response = await createEnvironment(h, client, body);
  expect(response.status).toBe(201);
  return (await response.json()) as Environment;
}

interface Routes {
  routes: {
    id: string;
    token_sha256: string;
    api: string;
    upstream_base_url: string;
    credential_file: string;
    models: { name: string }[];
    daily_token_budget: number;
  }[];
}

const routes = (h: Harness) =>
  JSON.parse(readFileSync(join(h.brokerDirectory, "routes.json"), "utf8")) as Routes;

const mode = (path: string) => statSync(path).mode & 0o777;

describe("keys", () => {
  test("are added, listed and deleted per owner without their secret", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const secret = providerSecret("router");
    const response = await addKey(h, client, { provider: "openrouter", label: "router", secret });
    expect(response.status).toBe(201);
    const text = await response.text();
    expect(text).not.toContain(secret);
    const key = JSON.parse(text) as ModelKey;
    expect(key).toMatchObject({ provider: "openrouter", label: "router", baseUrl: null });

    h.clock.now += 1000;
    const compatible = await addKey(h, client, {
      provider: "openai-compatible",
      label: "gateway",
      secret: ` ${secret}-2\n`,
      baseUrl: "https://llm.example/v1",
    });
    expect(((await compatible.json()) as ModelKey).baseUrl).toBe("https://llm.example/v1/");
    const listed = await call(h, "GET", "/api/v1/keys", undefined, client);
    expect(await listed.clone().text()).not.toContain(secret);
    expect(((await listed.json()) as KeyList).keys.map((k) => k.label)).toEqual([
      "default",
      "router",
      "gateway",
    ]);

    // Stored encrypted, bound to the row.
    const stored = h.db.query("SELECT secret FROM model_keys WHERE id = ?").get(key.id) as {
      secret: string;
    };
    expect(stored.secret).toStartWith("v1.");
    expect(stored.secret).not.toContain(secret);

    expect((await call(h, "DELETE", `/api/v1/keys/${key.id}`, undefined, client)).status).toBe(204);
    expect((await listKeys(h, client)).map((k) => k.label)).toEqual(["default", "gateway"]);
    expect((await call(h, "DELETE", `/api/v1/keys/${key.id}`, undefined, client)).status).toBe(404);
  });

  test("validate the request", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const valid = { provider: "anthropic", label: "main", secret: "sk-ant-fixture" };
    for (const body of [
      { ...valid, provider: "gemini" },
      { ...valid, provider: "constructor" },
      { ...valid, label: "" },
      { ...valid, label: "x".repeat(65) },
      { ...valid, secret: "" },
      { ...valid, secret: "two\nlines" },
      { ...valid, secret: 42 },
      { ...valid, baseUrl: "https://api.anthropic.com/v1" },
      { ...valid, provider: "openai-compatible" },
      { ...valid, provider: "openai-compatible", baseUrl: "ftp://llm.example/" },
      { ...valid, provider: "openai-compatible", baseUrl: "https://user:pw@llm.example/" },
      { ...valid, provider: "openai-compatible", baseUrl: "https://llm.example/v1?x=1" },
    ]) {
      expect((await addKey(h, client, body)).status).toBe(400);
    }
    expect(await listKeys(h, client)).toHaveLength(1);
  });

  test("of another owner cannot be listed, deleted or referenced", async () => {
    const h = harness();
    const owner = await newUser(h, "owner");
    const other = await newUser(h, "other", "admin");
    expect((await listKeys(h, other.client)).map((k) => k.id)).toEqual([other.client.keyId]);
    const path = `/api/v1/keys/${owner.client.keyId}`;
    expect((await call(h, "DELETE", path, undefined, other.client)).status).toBe(404);
    const borrowed = await createEnvironment(h, other.client, { keyId: owner.client.keyId });
    expect(borrowed.status).toBe(400);
    expect(h.docker.containers.size).toBe(0);
    expect(await listKeys(h, owner.client)).toHaveLength(1);
  });

  test("are never logged, returned or part of a container's generated configuration", async () => {
    const spies = (["log", "info", "warn", "error", "debug"] as const).map((method) =>
      spyOn(console, method),
    );
    try {
      const h = harness();
      const { client } = await newUser(h, "member");
      const secret = providerSecret("member");
      const env = await environment(h, client);
      const bodies = [
        await (await call(h, "GET", "/api/v1/keys", undefined, client)).text(),
        await (await call(h, "GET", "/api/v1/environments", undefined, client)).text(),
        await (await call(h, "GET", `/api/v1/environments/${env.id}`, undefined, client)).text(),
      ];
      h.docker.failing.add("createContainer");
      await createEnvironment(h, client, { name: "failing" });
      h.docker.failing.clear();
      await call(h, "DELETE", `/api/v1/keys/${client.keyId}`, undefined, client);

      for (const body of bodies) expect(body).not.toContain(secret);
      const container = [...h.docker.containers.values()][0];
      expect(JSON.stringify(container?.spec)).not.toContain(secret);
      for (const spy of spies) {
        for (const args of spy.mock.calls) expect(JSON.stringify(args)).not.toContain(secret);
      }
    } finally {
      for (const spy of spies) spy.mockRestore();
    }
  });
});

describe("broker files", () => {
  test("route each environment's token to its owner's key", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const env = await environment(h, client, { models: ["model-a", "model-b"] });
    const variables = envOf([...h.docker.containers.values()][0]?.spec.Env ?? []);
    const credentialFile = join(h.brokerDirectory, `${env.id}.key`);
    expect(routes(h)).toEqual({
      routes: [
        {
          id: env.id,
          token_sha256: sha256(variables.TENTAFLAKE_MODEL_API_KEY ?? ""),
          api: "anthropic",
          upstream_base_url: "https://api.anthropic.com/v1/",
          credential_file: credentialFile,
          models: [{ name: "model-a" }, { name: "model-b" }],
          daily_token_budget: 50_000,
        },
      ],
    });
    expect(readFileSync(credentialFile, "utf8")).toBe(providerSecret("member"));
    expect(mode(credentialFile)).toBe(0o600);
    expect(mode(join(h.brokerDirectory, "routes.json"))).toBe(0o600);
    expect(mode(h.brokerDirectory)).toBe(0o700);
    expect(readFileSync(join(h.brokerDirectory, "routes.json"), "utf8")).not.toContain(
      providerSecret("member"),
    );

    const compatible = (await (
      await addKey(h, client, {
        provider: "openai-compatible",
        label: "gateway",
        secret: "compatible-secret",
        baseUrl: "https://llm.example/v1",
      })
    ).json()) as ModelKey;
    await environment(h, client, { name: "other", keyId: compatible.id });
    expect(routes(h).routes.map((route) => [route.api, route.upstream_base_url])).toContainEqual([
      "openai",
      "https://llm.example/v1/",
    ]);
  });

  test("lose an environment's route and credential when it or its key is deleted", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const first = await environment(h, client);
    const second = await environment(h, client, { name: "second" });
    const file = (env: Environment) => join(h.brokerDirectory, `${env.id}.key`);
    expect(routes(h).routes).toHaveLength(2);

    await call(h, "DELETE", `/api/v1/environments/${first.id}`, undefined, client);
    expect(existsSync(file(first))).toBe(false);
    expect(routes(h).routes.map((route) => route.id)).toEqual([second.id]);

    await call(h, "DELETE", `/api/v1/keys/${client.keyId}`, undefined, client);
    expect(existsSync(file(second))).toBe(false);
    expect(routes(h).routes).toEqual([]);
    const fetched = await call(h, "GET", `/api/v1/environments/${second.id}`, undefined, client);
    expect(((await fetched.json()) as Environment).keyId).toBeNull();
  });

  test("roll a deletion back when they cannot be written", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const env = await environment(h, client);
    const before = routes(h);
    const failing = () =>
      spyOn(h.broker, "sync").mockImplementationOnce(() => {
        throw new Error("broker directory is full");
      });
    const errors = spyOn(console, "error").mockImplementation(() => {});
    try {
      failing();
      const key = `/api/v1/keys/${client.keyId}`;
      expect((await call(h, "DELETE", key, undefined, client)).status).toBe(500);
      failing();
      const path = `/api/v1/environments/${env.id}`;
      expect((await call(h, "DELETE", path, undefined, client)).status).toBe(500);
      // Nothing is deleted that the broker still serves; both deletions can be retried.
      expect(await listKeys(h, client)).toHaveLength(1);
      const fetched = (await (await call(h, "GET", path, undefined, client)).json()) as Environment;
      expect(fetched.keyId).toBe(client.keyId ?? "");
      expect(routes(h)).toEqual(before);

      expect((await call(h, "DELETE", key, undefined, client)).status).toBe(204);
      expect(routes(h).routes).toEqual([]);
      expect(existsSync(join(h.brokerDirectory, `${env.id}.key`))).toBe(false);
      expect((await call(h, "DELETE", path, undefined, client)).status).toBe(204);
    } finally {
      errors.mockRestore();
    }
  });

  test("are rebuilt from the database after the runtime directory is lost", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const env = await environment(h, client);
    const before = routes(h);
    rmSync(h.brokerDirectory, { recursive: true });
    new BrokerFiles(h.db, h.secrets, h.brokerDirectory).sync();
    expect(routes(h)).toEqual(before);
    expect(readFileSync(join(h.brokerDirectory, `${env.id}.key`), "utf8")).toBe(
      providerSecret("member"),
    );
  });
});
