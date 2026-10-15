import { describe, expect, test } from "bun:test";
import baseConfig from "../../images/runtime/opencode.json";
import type { Environment, EnvironmentList } from "../src/api";
import { BrokerFiles } from "../src/broker";
import { DockerError, dockerClient } from "../src/docker";
import { Environments, ensureNetworks } from "../src/environments";
import {
  brokerUrl,
  type Client,
  call,
  createEnvironment as create,
  envOf,
  FakeDocker,
  type Harness,
  harness,
  newUser,
} from "./helpers";

async function created(h: Harness, client: Client, body?: object): Promise<Environment> {
  const response = await create(h, client, body);
  expect(response.status).toBe(201);
  return (await response.json()) as Environment;
}

const list = async (h: Harness, client: Client) =>
  (
    (await (
      await call(h, "GET", "/api/v1/environments", undefined, client)
    ).json()) as EnvironmentList
  ).environments;

describe("environments", () => {
  test("run as hardened runsc containers with a workspace volume", async () => {
    const h = harness();
    const { client, user } = await newUser(h, "member");
    const env = await created(h, client);
    expect(env).toMatchObject({
      name: "dev",
      state: "running",
      networkMode: "model-only",
      keyId: client.keyId,
      models: ["fixture-model"],
      limits: { memoryMiB: 1024, cpus: 1.5, pids: 256, dailyTokens: 50_000 },
    });

    const [container] = [...h.docker.containers.values()];
    expect(container?.running).toBe(true);
    expect(container?.name).toBe(`tentaflake-${env.id}`);
    const spec = container?.spec;
    expect(spec?.Image).toBe("runtime:test");
    expect(spec?.User).toBe("1000:1000");
    expect(spec?.Labels).toEqual({ "io.tentaflake.environment": env.id });
    expect(spec?.HostConfig).toMatchObject({
      Runtime: "runsc",
      NetworkMode: "tentaflake-model-only",
      CapDrop: ["ALL"],
      SecurityOpt: ["no-new-privileges"],
      ReadonlyRootfs: true,
      Tmpfs: { "/tmp": "exec,mode=1777" },
      Mounts: [
        { Type: "volume", Source: `tentaflake-${env.id}-workspace`, Target: "/workspace" },
        { Type: "volume", Source: `tentaflake-${env.id}-home`, Target: "/home/agent" },
      ],
      Memory: 1024 * 2 ** 20,
      MemorySwap: 1024 * 2 ** 20,
      NanoCpus: 1_500_000_000,
      PidsLimit: 256,
    });
    expect([...h.docker.volumes]).toEqual([
      `tentaflake-${env.id}-workspace`,
      `tentaflake-${env.id}-home`,
    ]);

    const variables = envOf(spec?.Env ?? []);
    expect(JSON.parse(variables.OPENCODE_CONFIG_CONTENT ?? "")).toEqual({
      ...baseConfig,
      mcp: {},
      model: "broker/fixture-model",
      providers: {
        broker: {
          ...baseConfig.providers.broker,
          package: "aisdk:@ai-sdk/anthropic",
          models: { "fixture-model": { name: "fixture-model" } },
        },
      },
    });
    expect(variables.TENTAFLAKE_MODEL_BASE_URL).toBe(brokerUrl);
    expect(variables.TENTAFLAKE_MODEL_API_KEY).toMatch(/^[\w-]{43}$/);
    const password = variables.OPENCODE_SERVER_PASSWORD ?? "";
    expect(password).toMatch(/^[\w-]{43}$/);
    expect(h.environments.opencodePassword(user.id, env.id)).toBe(password);
    const stored = h.db.query("SELECT opencode_password FROM environments").get() as {
      opencode_password: string;
    };
    expect(stored.opencode_password).not.toContain(password);
  });

  test("get their own OpenCode password and the requested network", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    await created(h, client);
    const open = await created(h, client, { name: "web", networkMode: "open" });
    expect(open.networkMode).toBe("open");
    const [first, second] = [...h.docker.containers.values()];
    expect(second?.spec.HostConfig.NetworkMode).toBe("tentaflake-open");
    const password = (env: string[] = []) => envOf(env).OPENCODE_SERVER_PASSWORD;
    expect(password(first?.spec.Env)).not.toBe(password(second?.spec.Env));
  });

  test("go through stop, start and delete", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const env = await created(h, client);
    const path = `/api/v1/environments/${env.id}`;
    const [container] = [...h.docker.containers.values()];

    for (const [action, state, running] of [
      ["stop", "stopped", false],
      ["stop", "stopped", false],
      ["start", "running", true],
    ] as const) {
      const response = await call(h, "POST", `${path}/${action}`, undefined, client);
      expect(response.status).toBe(200);
      expect(((await response.json()) as Environment).state).toBe(state);
      expect(container?.running).toBe(running);
    }
    const fetched = await call(h, "GET", path, undefined, client);
    expect(((await fetched.json()) as Environment).state).toBe("running");
    expect(await list(h, client)).toHaveLength(1);

    expect((await call(h, "DELETE", path, undefined, client)).status).toBe(204);
    expect((await call(h, "GET", path, undefined, client)).status).toBe(404);
    expect(await list(h, client)).toEqual([]);
    expect(h.docker.containers.size).toBe(0);
    expect(h.docker.volumes.size).toBe(0);
  });

  test("of another user are invisible and immutable", async () => {
    const h = harness();
    const owner = await newUser(h, "owner");
    const other = await newUser(h, "other", "admin");
    const env = await created(h, owner.client);
    const path = `/api/v1/environments/${env.id}`;

    expect(await list(h, other.client)).toEqual([]);
    for (const [method, suffix] of [
      ["GET", ""],
      ["POST", "/stop"],
      ["POST", "/start"],
      ["DELETE", ""],
    ] as const) {
      const response = await call(h, method, `${path}${suffix}`, undefined, other.client);
      expect(response.status).toBe(404);
    }
    expect([...h.docker.containers.values()][0]?.running).toBe(true);
    expect(await list(h, owner.client)).toMatchObject([{ id: env.id, state: "running" }]);

    // Names are per owner.
    expect((await created(h, other.client)).name).toBe("dev");
  });

  test("validate the request", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    for (const body of [
      { name: undefined },
      { name: "" },
      { name: "Dev" },
      { name: "-dev" },
      { name: "dev-" },
      { name: "x".repeat(64) },
      { networkMode: "internet" },
      { networkMode: "constructor" },
      { networkMode: 1 },
      { keyId: undefined },
      { keyId: "no-such-key" },
      { models: undefined },
      { models: [] },
      { models: "fixture-model" },
      { models: ["fixture-model", "fixture-model"] },
      { models: [""] },
      { models: ["bad\nmodel"] },
    ]) {
      expect((await create(h, client, body)).status).toBe(400);
    }
    await created(h, client);
    expect((await create(h, client)).status).toBe(409);
    expect(h.docker.containers.size).toBe(1);
  });

  test("allow one lifecycle operation at a time", async () => {
    const h = harness();
    const { client, user } = await newUser(h, "member");
    const env = await created(h, client);
    const [stop, remove] = await Promise.allSettled([
      h.environments.stop(user.id, env.id),
      h.environments.delete(user.id, env.id),
    ]);
    expect(stop.status).toBe("fulfilled");
    expect(remove).toMatchObject({ status: "rejected", reason: { status: 409 } });
  });

  test("start a container that exited on its own while recorded as running", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const env = await created(h, client);
    const [container] = [...h.docker.containers.values()];
    if (container !== undefined) container.running = false;

    const response = await call(
      h,
      "POST",
      `/api/v1/environments/${env.id}/start`,
      undefined,
      client,
    );
    expect(response.status).toBe(200);
    expect(((await response.json()) as Environment).state).toBe("running");
    expect(container?.running).toBe(true);
  });
});

describe("Docker failures", () => {
  for (const method of ["createVolume", "createContainer", "startContainer"] as const) {
    test(`in ${method} leave neither a record nor Docker resources`, async () => {
      const h = harness();
      const { client } = await newUser(h, "member");
      h.docker.failing.add(method);
      const response = await create(h, client);
      expect(response.status).toBe(502);
      expect(await response.json()).toMatchObject({ error: "docker" });
      expect(await list(h, client)).toEqual([]);
      expect(h.docker.containers.size).toBe(0);
      expect(h.docker.volumes.size).toBe(0);

      h.docker.failing.clear();
      await created(h, client);
    });
  }

  test("in the cleanup leave a failed record that its owner can delete", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    h.docker.failing.add("startContainer");
    h.docker.failing.add("removeContainer");
    expect((await create(h, client)).status).toBe(502);
    const [env] = await list(h, client);
    expect(env?.state).toBe("failed");
    const start = await call(h, "POST", `/api/v1/environments/${env?.id}/start`, undefined, client);
    expect(start.status).toBe(409);

    h.docker.failing.clear();
    expect(
      (await call(h, "DELETE", `/api/v1/environments/${env?.id}`, undefined, client)).status,
    ).toBe(204);
    expect(h.docker.containers.size).toBe(0);
    expect(h.docker.volumes.size).toBe(0);
  });

  test("on stop and start keep the recorded state", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const env = await created(h, client);
    const path = `/api/v1/environments/${env.id}`;
    const state = async () =>
      ((await (await call(h, "GET", path, undefined, client)).json()) as Environment).state;

    h.docker.failing.add("stopContainer");
    expect((await call(h, "POST", `${path}/stop`, undefined, client)).status).toBe(502);
    expect(await state()).toBe("running");
    h.docker.failing.clear();
    expect((await call(h, "POST", `${path}/stop`, undefined, client)).status).toBe(200);

    h.docker.failing.add("startContainer");
    expect((await call(h, "POST", `${path}/start`, undefined, client)).status).toBe(502);
    expect(await state()).toBe("stopped");
  });

  test("on delete keep the record until container and volume are gone", async () => {
    const h = harness();
    const { client } = await newUser(h, "member");
    const env = await created(h, client);
    const path = `/api/v1/environments/${env.id}`;

    h.docker.failing.add("removeContainer");
    expect((await call(h, "DELETE", path, undefined, client)).status).toBe(502);
    expect(await list(h, client)).toMatchObject([{ state: "running" }]);
    expect(h.docker.containers.size).toBe(1);

    h.docker.failing.clear();
    h.docker.failing.add("removeVolume");
    expect((await call(h, "DELETE", path, undefined, client)).status).toBe(502);
    expect(await list(h, client)).toMatchObject([{ state: "failed" }]);
    expect(h.docker.containers.size).toBe(0);
    expect(h.docker.volumes.size).toBe(2);

    h.docker.failing.clear();
    expect((await call(h, "DELETE", path, undefined, client)).status).toBe(204);
    expect(await list(h, client)).toEqual([]);
    expect(h.docker.volumes.size).toBe(0);
  });

  test("an interrupted creation is marked failed on restart", async () => {
    const h = harness();
    const { client, user } = await newUser(h, "member");
    const env = await created(h, client);
    h.db.query("UPDATE environments SET state = 'creating'").run();
    const broker = new BrokerFiles(h.db, h.secrets, h.brokerDirectory);
    const restarted = new Environments(h.db, h.docker, h.secrets, broker, {
      image: "runtime:test",
      limits: env.limits,
      brokerUrl,
    });
    expect(restarted.get(user.id, env.id).state).toBe("failed");
  });
});

describe("Docker client", () => {
  test("reports an unreachable socket as a Docker error", async () => {
    const docker = dockerClient("/nonexistent/docker.sock");
    await expect(docker.createVolume("workspace", {})).rejects.toBeInstanceOf(DockerError);
  });
});

describe("networks", () => {
  test("are created once, model-only without a route out", async () => {
    const docker = new FakeDocker();
    expect(await ensureNetworks(docker)).toBe("192.0.2.1");
    await ensureNetworks(docker);
    expect([...docker.networks]).toEqual([
      ["tentaflake-model-only", true],
      ["tentaflake-open", false],
    ]);
  });

  test("an existing model-only network with a route out is refused", async () => {
    const docker = new FakeDocker();
    docker.networks.set("tentaflake-model-only", false);
    await expect(ensureNetworks(docker)).rejects.toThrow("tentaflake-model-only");
  });
});
