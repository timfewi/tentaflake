import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Environment } from "../src/api";
import { Auth } from "../src/auth";
import { BrokerFiles } from "../src/broker";
import { randomToken, Secrets } from "../src/crypto";
import { openDatabase } from "../src/db";
import { dockerClient } from "../src/docker";
import { containerName, Environments, ensureNetworks } from "../src/environments";
import { Keys } from "../src/keys";

// Real Docker with runsc, the runtime image named by TENTAFLAKE_INTEGRATION_IMAGE
// and the broker binary named by TENTAFLAKE_INTEGRATION_BROKER;
// `just control-integration` runs it. No real provider and no real key.
const image = process.env.TENTAFLAKE_INTEGRATION_IMAGE ?? "";
const brokerBinary = process.env.TENTAFLAKE_INTEGRATION_BROKER ?? "";
const socket = process.env.TENTAFLAKE_INTEGRATION_DOCKER_SOCKET ?? "/var/run/docker.sock";
const brokerPort = 7810;

interface ContainerInfo {
  State: { Running: boolean };
  Config: { User: string; Labels: Record<string, string> };
  HostConfig: {
    Runtime: string;
    ReadonlyRootfs: boolean;
    CapDrop: string[];
    SecurityOpt: string[];
  };
  NetworkSettings: { Networks: Record<string, { IPAddress: string }> };
}

function engine(method: string, path: string, body?: unknown): Promise<Response> {
  return fetch(`http://docker/v1.44${path}`, {
    method,
    unix: socket,
    ...(body === undefined
      ? {}
      : { headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
  });
}

async function inspect(path: string): Promise<{ status: number; info: ContainerInfo }> {
  const response = await engine("GET", path);
  return { status: response.status, info: (await response.json()) as ContainerInfo };
}

/** Runs a shell script in the container as its user; returns exit code and output. */
async function exec(container: string, script: string): Promise<{ code: number; output: string }> {
  const created = await engine("POST", `/containers/${container}/exec`, {
    AttachStdout: true,
    AttachStderr: true,
    Tty: true,
    Cmd: ["sh", "-c", script],
  });
  const { Id } = (await created.json()) as { Id: string };
  const output = await (
    await engine("POST", `/exec/${Id}/start`, { Detach: false, Tty: true })
  ).text();
  for (;;) {
    const state = (await (await engine("GET", `/exec/${Id}/json`)).json()) as {
      Running: boolean;
      ExitCode: number;
    };
    if (!state.Running) return { code: state.ExitCode, output };
    await Bun.sleep(200);
  }
}

/** Retries until the request answers. */
async function reach(url: string, headers: Record<string, string> = {}): Promise<Response> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await fetch(url, { headers, signal: AbortSignal.timeout(5_000) });
    } catch (error) {
      if (attempt >= 60) throw error;
      await Bun.sleep(1_000);
    }
  }
}

describe.skipIf(image === "" || brokerBinary === "")("with Docker, runsc and the broker", () => {
  const db = openDatabase(":memory:");
  const secrets = new Secrets(randomBytes(32));
  const docker = dockerClient(socket);
  const providerKey = `fake-provider-key-${randomToken(12)}`;
  const received: { path: string; authorization: string | null }[] = [];
  let dir: string;
  let provider: ReturnType<typeof Bun.serve>;
  let broker: ReturnType<typeof Bun.spawn> | undefined;
  let environments: Environments;
  let ownerId: string;
  let keyId: string;

  beforeAll(async () => {
    dir = mkdtempSync(join(tmpdir(), "tentaflake-integration-"));
    // The fake provider listens on host loopback: the broker reaches it, containers cannot.
    provider = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch(request) {
        received.push({
          path: new URL(request.url).pathname,
          authorization: request.headers.get("authorization"),
        });
        return Response.json({
          id: "fake-completion",
          object: "chat.completion",
          choices: [{ index: 0, message: { role: "assistant", content: "hi" } }],
          usage: { prompt_tokens: 3, completion_tokens: 1, total_tokens: 4 },
        });
      },
    });
    const gateway = await ensureNetworks(docker);
    writeFileSync(
      join(dir, "broker.json"),
      JSON.stringify({
        listen: `${gateway}:${brokerPort}`,
        routes_file: join(dir, "broker", "routes.json"),
        audit_file: join(dir, "audit.jsonl"),
        budget_state_file: join(dir, "budget.json"),
        allow_plain_http_for_tests: true,
      }),
    );
    broker = Bun.spawn([brokerBinary, "--config", join(dir, "broker.json")], {
      stdout: "inherit",
      stderr: "inherit",
    });
    expect((await reach(`http://${gateway}:${brokerPort}/healthz`)).status).toBe(200);

    const files = new BrokerFiles(db, secrets, join(dir, "broker"));
    const auth = new Auth(db, secrets, {
      setupTokenFile: "/nonexistent",
      sessionIdleMinutes: 60,
      sessionAbsoluteHours: 12,
    });
    ownerId = (await auth.createUser("integration", "integration password", "admin")).user.id;
    keyId = new Keys(db, secrets, files).add(ownerId, {
      provider: "openai-compatible",
      label: "fake provider",
      secret: providerKey,
      baseUrl: `http://127.0.0.1:${provider.port}/v1`,
    }).id;
    const limits = { memoryMiB: 1024, cpus: 1, pids: 256, dailyTokens: 1_000_000 };
    const brokerUrl = `http://${gateway}:${brokerPort}/v1`;
    environments = new Environments(db, docker, secrets, files, { image, limits, brokerUrl });
  }, 120_000);

  afterAll(async () => {
    for (const env of environments?.list(ownerId) ?? []) {
      await docker.removeContainer(containerName(env.id)).catch(() => {});
      await docker.removeVolume(`tentaflake-${env.id}-workspace`).catch(() => {});
    }
    broker?.kill();
    await provider?.stop(true);
    if (dir !== undefined) rmSync(dir, { recursive: true, force: true });
  });

  const create = (name: string, networkMode: "model-only" | "open" = "model-only") =>
    environments.create(ownerId, { name, networkMode, keyId, models: ["fixture-model"] });

  test("create, running, stop, start and delete", async () => {
    const env = await create("lifecycle");
    const container = `/containers/${containerName(env.id)}/json`;
    expect(env.state).toBe("running");
    const { info } = await inspect(container);
    expect(info.State.Running).toBe(true);
    expect(info.HostConfig).toMatchObject({
      Runtime: "runsc",
      ReadonlyRootfs: true,
      CapDrop: ["ALL"],
      SecurityOpt: ["no-new-privileges"],
    });
    expect(info.Config.User).toBe("1000:1000");
    expect(info.Config.Labels["io.tentaflake.environment"]).toBe(env.id);

    // OpenCode answers on the model-only network, with the generated password and configuration.
    const address = info.NetworkSettings.Networks["tentaflake-model-only"]?.IPAddress;
    expect(address).toBeTruthy();
    const password = environments.opencodePassword(ownerId, env.id);
    const base = `http://${address}:4096/api`;
    const authorization = { authorization: `Basic ${btoa(`opencode:${password}`)}` };
    expect((await reach(`${base}/info`, authorization)).status).toBe(200);
    expect((await reach(`${base}/info`)).status).toBe(401);
    const documents = (await (await reach(`${base}/config`, authorization)).json()) as {
      type: string;
      path?: string;
      info?: { share?: string; update?: string; model?: unknown };
    }[];
    expect(documents).toContainEqual(
      expect.objectContaining({
        type: "document",
        info: expect.objectContaining({
          share: "disabled",
          update: "disable",
          // OpenCode 2.0.29 reports "broker/fixture-model" parsed.
          model: expect.objectContaining({ providerID: "broker", model: "fixture-model" }),
        }),
      }),
    );

    expect((await environments.stop(ownerId, env.id)).state).toBe("stopped");
    expect((await inspect(container)).info.State.Running).toBe(false);
    expect((await environments.start(ownerId, env.id)).state).toBe("running");
    expect((await inspect(container)).info.State.Running).toBe(true);

    await environments.delete(ownerId, env.id);
    expect(environments.list(ownerId)).toEqual([]);
    expect((await inspect(container)).status).toBe(404);
    expect((await inspect(`/volumes/tentaflake-${env.id}-workspace`)).status).toBe(404);
  }, 240_000);

  test("model calls reach the provider through the broker, and nothing else is reachable", async () => {
    const modelOnly = await create("model-only");
    const open = await create("open", "open");
    const call = (token: string) =>
      `curl -sS -m 30 -w ' %{http_code}' -H "authorization: Bearer ${token}" ` +
      `-H 'content-type: application/json' ` +
      `-d '{"model":"fixture-model","messages":[{"role":"user","content":"hi"}]}' ` +
      `"$TENTAFLAKE_MODEL_BASE_URL/chat/completions"`;

    for (const env of [modelOnly, open] as Environment[]) {
      const name = containerName(env.id);
      const answer = await exec(name, call("$TENTAFLAKE_MODEL_API_KEY"));
      expect(answer.output).toContain("fake-completion");
      expect(answer.output.trim()).toEndWith(" 200");
      const refused = await exec(name, call("not-the-environment-token"));
      expect(refused.output.trim()).toEndWith(" 401");

      // The provider key is nowhere in the container's configuration or environment.
      const configuration = await (await engine("GET", `/containers/${name}/json`)).text();
      expect(configuration).not.toContain(providerKey);
      const environment = await exec(name, "env; cat /proc/1/environ");
      expect(environment.output).toContain("TENTAFLAKE_MODEL_API_KEY=");
      expect(environment.output).not.toContain(providerKey);
    }
    expect(received).toEqual([
      { path: "/v1/chat/completions", authorization: `Bearer ${providerKey}` },
      { path: "/v1/chat/completions", authorization: `Bearer ${providerKey}` },
    ]);

    // model-only: no route out, by address or by name; the provider on host loopback is out of reach.
    const name = containerName(modelOnly.id);
    for (const target of [
      "http://1.1.1.1/",
      "https://example.com/",
      `http://127.0.0.1:${provider.port}/`,
    ]) {
      expect((await exec(name, `curl -sS -m 5 -o /dev/null ${target}`)).code).not.toBe(0);
    }

    for (const env of [modelOnly, open]) {
      await environments.delete(ownerId, env.id);
      expect(existsSync(join(dir, "broker", `${env.id}.key`))).toBe(false);
    }
  }, 300_000);
});
