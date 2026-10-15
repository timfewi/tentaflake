import type { Database } from "bun:sqlite";
import { randomUUID } from "node:crypto";
import baseConfig from "../../images/runtime/opencode.json";
import type {
  CreateEnvironmentRequest,
  Environment,
  EnvironmentState,
  Limits,
  NetworkMode,
} from "./api";
import type { Config } from "./config";
import { randomToken, type Secrets } from "./crypto";
import type { ContainerSpec, Docker } from "./docker";
import { HttpError } from "./http";

/** The Docker networks the service owns; each environment joins one of them. */
export const networks: Record<NetworkMode, { name: string; internal: boolean }> = {
  // No route out of the host; the broker joins this network.
  "model-only": { name: "tentaflake-model-only", internal: true },
  open: { name: "tentaflake-open", internal: false },
};

/** Creates missing networks and refuses an existing one with the wrong `internal` flag. */
export async function ensureNetworks(docker: Docker): Promise<void> {
  for (const { name, internal } of Object.values(networks)) {
    const existing = await docker.network(name);
    if (existing === null) {
      await docker.createNetwork(name, internal);
    } else if (existing.internal !== internal) {
      throw new Error(
        `Docker network ${name} exists with internal=${existing.internal}, not ${internal}`,
      );
    }
  }
}

// The generated per-environment OpenCode configuration: the runtime image's
// base configuration, passed as OPENCODE_CONFIG_CONTENT. OpenCode ranks that
// layer above an opencode.json written into /workspace (observed on 2.0.29),
// so the agent cannot switch sharing, updates or permissions back on. The
// model provider and MCP servers of an environment join it later.
const opencodeConfig = JSON.stringify(baseConfig);

export const environmentLabel = "io.tentaflake.environment";
export const containerName = (id: string) => `tentaflake-${id}`;
const namePattern = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

interface EnvironmentRow {
  id: string;
  owner_id: string;
  name: string;
  state: EnvironmentState;
  container_id: string | null;
  workspace_volume: string;
  network_mode: NetworkMode;
  limits: string;
  opencode_password: string;
  created_at: number;
  last_activity_at: number;
}

const toEnvironment = (row: EnvironmentRow): Environment => ({
  id: row.id,
  name: row.name,
  state: row.state,
  networkMode: row.network_mode,
  limits: JSON.parse(row.limits) as Limits,
  createdAt: new Date(row.created_at).toISOString(),
  lastActivityAt: new Date(row.last_activity_at).toISOString(),
});

const passwordContext = (id: string) => `environments.opencode_password:${id}`;

/**
 * The environment lifecycle. Every method takes the calling user's id and
 * sees only that user's environments; others answer 404. A record exists
 * while Docker may hold resources for it, and a failed Docker call is an error.
 */
export class Environments {
  private readonly busy = new Set<string>();

  constructor(
    private readonly db: Database,
    private readonly docker: Docker,
    private readonly secrets: Secrets,
    private readonly config: Pick<Config, "image" | "limits">,
    private readonly now: () => number = Date.now,
  ) {
    // A restart during a creation leaves it unfinished; its owner can delete it.
    db.run("UPDATE environments SET state = 'failed' WHERE state = 'creating'");
  }

  list(ownerId: string): Environment[] {
    const rows = this.db
      .query("SELECT * FROM environments WHERE owner_id = ? ORDER BY created_at, id")
      .all(ownerId) as EnvironmentRow[];
    return rows.map(toEnvironment);
  }

  get(ownerId: string, id: string): Environment {
    return toEnvironment(this.row(ownerId, id));
  }

  /** The password of the environment's OpenCode server. */
  opencodePassword(ownerId: string, id: string): string {
    return this.secrets.decrypt(this.row(ownerId, id).opencode_password, passwordContext(id));
  }

  async create(ownerId: string, request: CreateEnvironmentRequest): Promise<Environment> {
    const { name, networkMode = "model-only" } = request;
    if (!namePattern.test(name)) {
      throw new HttpError(
        400,
        "invalid_request",
        "name must be 1-63 lowercase letters, digits or inner hyphens",
      );
    }
    if (!Object.hasOwn(networks, networkMode)) {
      throw new HttpError(400, "invalid_request", "networkMode must be model-only or open");
    }
    if (
      this.db
        .query("SELECT 1 FROM environments WHERE owner_id = ? AND name = ?")
        .get(ownerId, name) !== null
    ) {
      throw new HttpError(409, "conflict", "an environment with this name exists");
    }
    const id = randomUUID();
    const password = randomToken();
    const now = this.now();
    const row: EnvironmentRow = {
      id,
      owner_id: ownerId,
      name,
      state: "creating",
      container_id: null,
      workspace_volume: `tentaflake-${id}-workspace`,
      network_mode: networkMode,
      limits: JSON.stringify(this.config.limits),
      opencode_password: this.secrets.encrypt(password, passwordContext(id)),
      created_at: now,
      last_activity_at: now,
    };
    this.db
      .query(
        `INSERT INTO environments (id, owner_id, name, state, container_id, workspace_volume,
           network_mode, limits, opencode_password, created_at, last_activity_at)
         VALUES ($id, $owner_id, $name, $state, $container_id, $workspace_volume,
           $network_mode, $limits, $opencode_password, $created_at, $last_activity_at)`,
      )
      .run({ ...row });

    try {
      await this.docker.createVolume(row.workspace_volume, { [environmentLabel]: id });
      const containerId = await this.docker.createContainer(
        containerName(id),
        this.containerSpec(row, password),
      );
      this.db.query("UPDATE environments SET container_id = ? WHERE id = ?").run(containerId, id);
      await this.docker.startContainer(containerId);
    } catch (error) {
      await this.discard(row);
      throw error;
    }
    this.setState(id, "running");
    return this.get(ownerId, id);
  }

  async start(ownerId: string, id: string): Promise<Environment> {
    await this.exclusive(ownerId, id, async (row) => {
      if (row.state === "running") return;
      if (row.state !== "stopped" || row.container_id === null) throw notNow(row.state, "start");
      await this.docker.startContainer(row.container_id);
      this.setState(id, "running");
    });
    return this.get(ownerId, id);
  }

  async stop(ownerId: string, id: string): Promise<Environment> {
    await this.exclusive(ownerId, id, async (row) => {
      if (row.state === "stopped") return;
      if (row.state !== "running" || row.container_id === null) throw notNow(row.state, "stop");
      await this.docker.stopContainer(row.container_id);
      this.setState(id, "stopped");
    });
    return this.get(ownerId, id);
  }

  /** Removes the container, then the workspace volume, then the record. */
  async delete(ownerId: string, id: string): Promise<void> {
    await this.exclusive(ownerId, id, async (row) => {
      if (row.state === "creating") throw notNow(row.state, "delete");
      await this.docker.removeContainer(containerName(id));
      // The container is gone; if the volume stays, a failed record remains to retry.
      this.db
        .query("UPDATE environments SET state = 'failed', container_id = NULL WHERE id = ?")
        .run(id);
      await this.docker.removeVolume(row.workspace_volume);
      this.db.query("DELETE FROM environments WHERE id = ?").run(id);
    });
  }

  private row(ownerId: string, id: string): EnvironmentRow {
    const row = this.db
      .query("SELECT * FROM environments WHERE id = ? AND owner_id = ?")
      .get(id, ownerId) as EnvironmentRow | null;
    if (row === null) throw new HttpError(404, "not_found", "no such environment");
    return row;
  }

  /** Runs one lifecycle operation at a time per environment. */
  private async exclusive(
    ownerId: string,
    id: string,
    operation: (row: EnvironmentRow) => Promise<void>,
  ): Promise<void> {
    const row = this.row(ownerId, id);
    if (this.busy.has(id)) {
      throw new HttpError(409, "conflict", "another operation on this environment is in progress");
    }
    this.busy.add(id);
    try {
      await operation(row);
    } finally {
      this.busy.delete(id);
    }
  }

  /** After a failed creation: removes the record only once Docker holds nothing for it. */
  private async discard(row: EnvironmentRow): Promise<void> {
    try {
      await this.docker.removeContainer(containerName(row.id));
      await this.docker.removeVolume(row.workspace_volume);
      this.db.query("DELETE FROM environments WHERE id = ?").run(row.id);
    } catch (error) {
      console.error(`environment ${row.id}: cleanup after a failed creation failed: ${error}`);
      this.setState(row.id, "failed");
    }
  }

  private setState(id: string, state: EnvironmentState): void {
    this.db
      .query("UPDATE environments SET state = ?, last_activity_at = ? WHERE id = ?")
      .run(state, this.now(), id);
  }

  private containerSpec(row: EnvironmentRow, password: string): ContainerSpec {
    const { memoryMiB, cpus, pids } = this.config.limits;
    const memory = memoryMiB * 1024 * 1024;
    return {
      Image: this.config.image,
      User: "1000:1000",
      Env: [`OPENCODE_SERVER_PASSWORD=${password}`, `OPENCODE_CONFIG_CONTENT=${opencodeConfig}`],
      Labels: { [environmentLabel]: row.id },
      HostConfig: {
        Runtime: "runsc",
        NetworkMode: networks[row.network_mode].name,
        CapDrop: ["ALL"],
        SecurityOpt: ["no-new-privileges"],
        ReadonlyRootfs: true,
        Tmpfs: { "/tmp": "exec,mode=1777", "/home/agent": "exec,uid=1000,gid=1000,mode=0700" },
        Mounts: [{ Type: "volume", Source: row.workspace_volume, Target: "/workspace" }],
        Memory: memory,
        MemorySwap: memory,
        NanoCpus: Math.round(cpus * 1e9),
        PidsLimit: pids,
      },
    };
  }
}

const notNow = (state: EnvironmentState, action: string) =>
  new HttpError(409, "conflict", `cannot ${action} an environment that is ${state}`);
