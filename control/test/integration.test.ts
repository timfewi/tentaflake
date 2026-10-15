import { describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { Auth } from "../src/auth";
import { Secrets } from "../src/crypto";
import { openDatabase } from "../src/db";
import { dockerClient } from "../src/docker";
import { containerName, Environments, ensureNetworks } from "../src/environments";

// Real Docker with runsc and the runtime image named by
// TENTAFLAKE_INTEGRATION_IMAGE; `just control-integration` runs it.
const image = process.env.TENTAFLAKE_INTEGRATION_IMAGE ?? "";
const socket = process.env.TENTAFLAKE_INTEGRATION_DOCKER_SOCKET ?? "/var/run/docker.sock";

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

async function inspect(path: string): Promise<{ status: number; info: ContainerInfo }> {
  const response = await fetch(`http://docker/v1.44${path}`, { unix: socket });
  return { status: response.status, info: (await response.json()) as ContainerInfo };
}

/** The response of the environment's OpenCode server, once it answers. */
async function opencode(url: string, password?: string): Promise<Response> {
  const headers: Record<string, string> =
    password === undefined ? {} : { authorization: `Basic ${btoa(`opencode:${password}`)}` };
  for (let attempt = 0; ; attempt++) {
    try {
      return await fetch(url, { headers, signal: AbortSignal.timeout(5_000) });
    } catch (error) {
      if (attempt >= 60) throw error;
      await Bun.sleep(1_000);
    }
  }
}

describe.skipIf(image === "")("with Docker, runsc and the runtime image", () => {
  test("create, running, stop, start and delete", async () => {
    const db = openDatabase(":memory:");
    const secrets = new Secrets(randomBytes(32));
    const docker = dockerClient(socket);
    await ensureNetworks(docker);
    const auth = new Auth(db, secrets, {
      setupTokenFile: "/nonexistent",
      sessionIdleMinutes: 60,
      sessionAbsoluteHours: 12,
    });
    const { user } = await auth.createUser("integration", "integration password", "admin");
    const limits = { memoryMiB: 1024, cpus: 1, pids: 256 };
    const environments = new Environments(db, docker, secrets, { image, limits });

    const env = await environments.create(user.id, { name: "integration" });
    const container = `/containers/${containerName(env.id)}/json`;
    try {
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
      const password = environments.opencodePassword(user.id, env.id);
      const base = `http://${address}:4096/api`;
      expect((await opencode(`${base}/info`, password)).status).toBe(200);
      expect((await opencode(`${base}/info`)).status).toBe(401);
      const documents = (await (await opencode(`${base}/config`, password)).json()) as {
        type: string;
        path?: string;
        info?: { share?: string; update?: string };
      }[];
      expect(documents).toContainEqual(
        expect.objectContaining({
          type: "document",
          info: expect.objectContaining({ share: "disabled", update: "disable" }),
        }),
      );
      expect(documents.some((doc) => doc.type === "document" && doc.path === undefined)).toBe(true);

      expect((await environments.stop(user.id, env.id)).state).toBe("stopped");
      expect((await inspect(container)).info.State.Running).toBe(false);
      expect((await environments.start(user.id, env.id)).state).toBe("running");
      expect((await inspect(container)).info.State.Running).toBe(true);

      await environments.delete(user.id, env.id);
      expect(environments.list(user.id)).toEqual([]);
      expect((await inspect(container)).status).toBe(404);
      expect((await inspect(`/volumes/tentaflake-${env.id}-workspace`)).status).toBe(404);
    } finally {
      await docker.removeContainer(containerName(env.id)).catch(() => {});
      await docker.removeVolume(`tentaflake-${env.id}-workspace`).catch(() => {});
    }
  }, 240_000);
});
