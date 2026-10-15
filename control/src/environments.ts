import type { Database } from "bun:sqlite";
import { randomUUID } from "node:crypto";
import baseConfig from "../../images/runtime/opencode.json";
import type {
  CreateEnvironmentRequest,
  Environment,
  EnvironmentState,
  Limits,
  McpServer,
  McpServerDefinition,
  McpServerResponse,
  NetworkMode,
  Provider,
} from "./api";
import { type BrokerFiles, brokerTokenContext } from "./broker";
import type { Config } from "./config";
import { randomToken, type Secrets } from "./crypto";
import type { ContainerSpec, Docker } from "./docker";
import { HttpError } from "./http";
import { providers } from "./keys";
import { mcpNamePattern, mcpSecretWarning, opencodeMcp, parseMcpServer } from "./mcp";

/** The Docker networks the service owns; each environment joins one of them. */
export const networks: Record<NetworkMode, { name: string; internal: boolean }> = {
  // No route out of the host; the broker joins this network.
  "model-only": { name: "tentaflake-model-only", internal: true },
  open: { name: "tentaflake-open", internal: false },
};

/**
 * Creates missing networks and refuses an existing one with the wrong
 * `internal` flag. Returns the model-only network's gateway: the host address
 * the broker listens on, which environments on either network reach.
 */
export async function ensureNetworks(docker: Docker): Promise<string> {
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
  const gateway = (await docker.network(networks["model-only"].name))?.gateway;
  if (!gateway) throw new Error(`Docker network ${networks["model-only"].name} has no gateway`);
  return gateway;
}

/**
 * The generated per-environment OpenCode configuration, passed as
 * OPENCODE_CONFIG_CONTENT: the runtime image's base configuration with the
 * environment's provider package, models and MCP servers. OpenCode ranks that
 * layer above an opencode.json written into /workspace (observed on 2.0.29),
 * so the agent cannot switch sharing, updates or permissions back on. The
 * base configuration reads the broker URL and token from the environment; no
 * provider key is ever part of it.
 */
export function opencodeConfig(
  provider: Provider | null,
  models: string[],
  mcp: Record<string, object>,
): string {
  const config: Record<string, unknown> = { ...baseConfig, mcp };
  if (provider !== null) {
    config.model = `broker/${models[0]}`;
    config.providers = {
      broker: {
        ...baseConfig.providers.broker,
        package: providers[provider].package,
        models: Object.fromEntries(models.map((model) => [model, { name: model }])),
      },
    };
  }
  return JSON.stringify(config);
}

export const environmentLabel = "io.tentaflake.environment";
export const containerName = (id: string) => `tentaflake-${id}`;
/** The agent's home directory: OpenCode keeps its sessions there, so they outlive the container. */
export const homeVolume = (id: string) => `tentaflake-${id}-home`;
/** The port of the OpenCode server in every environment. */
export const opencodePort = 4096;
const namePattern = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;
const invalid = (message: string) => new HttpError(400, "invalid_request", message);

interface EnvironmentRow {
  id: string;
  owner_id: string;
  name: string;
  state: EnvironmentState;
  container_id: string | null;
  workspace_volume: string;
  network_mode: NetworkMode;
  key_id: string | null;
  models: string;
  limits: string;
  opencode_password: string;
  broker_token: string | null;
  created_at: number;
  last_activity_at: number;
}

const toEnvironment = (row: EnvironmentRow): Environment => ({
  id: row.id,
  name: row.name,
  state: row.state,
  networkMode: row.network_mode,
  keyId: row.key_id,
  models: JSON.parse(row.models) as string[],
  limits: JSON.parse(row.limits) as Limits,
  createdAt: new Date(row.created_at).toISOString(),
  lastActivityAt: new Date(row.last_activity_at).toISOString(),
});

const passwordContext = (id: string) => `environments.opencode_password:${id}`;
const mcpSecretsContext = (id: string, name: string) => `mcp_servers.secrets:${id}/${name}`;

/** A live connection into an environment: an event stream or a terminal. */
export interface Connection {
  terminal: boolean;
  close(): void;
}

interface McpServerRow {
  environment_id: string;
  name: string;
  definition: string;
  secrets: string | null;
}

/**
 * The environment lifecycle. Every method takes the calling user's id and
 * sees only that user's environments; others answer 404. A record exists
 * while Docker may hold resources for it, and a failed Docker call is an error.
 */
export class Environments {
  private readonly busy = new Set<string>();
  private readonly connections = new Map<string, Set<Connection>>();

  constructor(
    private readonly db: Database,
    private readonly docker: Docker,
    private readonly secrets: Secrets,
    private readonly broker: BrokerFiles,
    /** `brokerUrl`: the broker's base URL as environments reach it. */
    private readonly config: Pick<Config, "image" | "limits"> & {
      brokerUrl: string;
      opencodePort?: number;
    },
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

  /**
   * The base URL and password of the OpenCode server in a running
   * environment, reached on the container's address; null if the container
   * does not run.
   */
  async opencode(ownerId: string, id: string): Promise<{ url: string; password: string } | null> {
    const row = this.row(ownerId, id);
    if (row.state !== "running" || row.container_id === null) return null;
    const address = await this.docker.containerAddress(
      row.container_id,
      networks[row.network_mode].name,
    );
    if (address === null) return null;
    return {
      url: `http://${address}:${this.config.opencodePort ?? opencodePort}`,
      password: this.secrets.decrypt(row.opencode_password, passwordContext(id)),
    };
  }

  /** Records activity in a running environment; written at most every 10 seconds. */
  touch(id: string): void {
    const now = this.now();
    this.db
      .query(
        "UPDATE environments SET last_activity_at = ? WHERE id = ? AND state = 'running' AND last_activity_at < ?",
      )
      .run(now, id, now - 10_000);
  }

  /** Running environments, of every owner, without activity since `time`. */
  idleSince(time: number): { id: string; ownerId: string }[] {
    return this.db
      .query(
        "SELECT id, owner_id AS ownerId FROM environments WHERE state = 'running' AND last_activity_at < ?",
      )
      .all(time) as { id: string; ownerId: string }[];
  }

  /**
   * Tracks a connection into a running environment until its returned release
   * is called. Stopping or replacing the container closes it first, so that it
   * ends with the environment rather than with OpenCode; a connection to an
   * environment that does not run or is being changed is closed at once.
   */
  connect(id: string, connection: Connection): () => void {
    const row = this.db.query("SELECT state FROM environments WHERE id = ?").get(id) as {
      state: EnvironmentState;
    } | null;
    if (row?.state !== "running" || this.busy.has(id)) {
      connection.close();
      return () => {};
    }
    const open = this.connections.get(id) ?? new Set();
    this.connections.set(id, open.add(connection));
    return () => {
      open.delete(connection);
      if (open.size === 0 && this.connections.get(id) === open) this.connections.delete(id);
    };
  }

  /** Whether a terminal into the environment is open. */
  terminalOpen(id: string): boolean {
    return [...(this.connections.get(id) ?? [])].some((connection) => connection.terminal);
  }

  async create(ownerId: string, request: CreateEnvironmentRequest): Promise<Environment> {
    const { name, networkMode = "model-only", keyId, models } = request;
    if (!namePattern.test(name)) {
      throw invalid("name must be 1-63 lowercase letters, digits or inner hyphens");
    }
    if (!Object.hasOwn(networks, networkMode)) {
      throw invalid("networkMode must be model-only or open");
    }
    if (
      models.length === 0 ||
      models.length > 32 ||
      new Set(models).size !== models.length ||
      !models.every((model) => model !== "" && model.length <= 200 && !/\p{Cc}/u.test(model))
    ) {
      throw invalid("models must be 1-32 distinct model names");
    }
    if (
      this.db
        .query("SELECT 1 FROM model_keys WHERE id = ? AND owner_id = ?")
        .get(keyId, ownerId) === null
    ) {
      throw invalid("keyId must name one of your keys");
    }
    if (
      this.db
        .query("SELECT 1 FROM environments WHERE owner_id = ? AND name = ?")
        .get(ownerId, name) !== null
    ) {
      throw new HttpError(409, "conflict", "an environment with this name exists");
    }
    const id = randomUUID();
    const now = this.now();
    const row: EnvironmentRow = {
      id,
      owner_id: ownerId,
      name,
      state: "creating",
      container_id: null,
      workspace_volume: `tentaflake-${id}-workspace`,
      network_mode: networkMode,
      key_id: keyId,
      models: JSON.stringify(models),
      limits: JSON.stringify(this.config.limits),
      opencode_password: this.secrets.encrypt(randomToken(), passwordContext(id)),
      broker_token: this.secrets.encrypt(randomToken(), brokerTokenContext(id)),
      created_at: now,
      last_activity_at: now,
    };
    this.broker.commit(() =>
      this.db
        .query(
          `INSERT INTO environments (id, owner_id, name, state, container_id, workspace_volume,
             network_mode, key_id, models, limits, opencode_password, broker_token, created_at,
             last_activity_at)
           VALUES ($id, $owner_id, $name, $state, $container_id, $workspace_volume,
             $network_mode, $key_id, $models, $limits, $opencode_password, $broker_token,
             $created_at, $last_activity_at)`,
        )
        .run({ ...row }),
    );

    try {
      await this.docker.createVolume(row.workspace_volume, { [environmentLabel]: id });
      await this.docker.createVolume(homeVolume(id), { [environmentLabel]: id });
      const containerId = await this.docker.createContainer(
        containerName(id),
        this.containerSpec(row),
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

  start(ownerId: string, id: string): Promise<Environment> {
    return this.switchTo(ownerId, id, "running");
  }

  stop(ownerId: string, id: string): Promise<Environment> {
    return this.switchTo(ownerId, id, "stopped");
  }

  /** Removes the container, then the workspace and home volumes, then the record. */
  async delete(ownerId: string, id: string): Promise<void> {
    await this.exclusive(ownerId, id, async (row) => {
      if (row.state === "creating") throw notNow(row.state, "delete");
      this.disconnect(id);
      await this.docker.removeContainer(containerName(id));
      // The container is gone; if a volume stays, a failed record remains to retry.
      this.db
        .query("UPDATE environments SET state = 'failed', container_id = NULL WHERE id = ?")
        .run(id);
      await this.docker.removeVolume(row.workspace_volume);
      await this.docker.removeVolume(homeVolume(id));
      this.broker.commit(() => this.db.query("DELETE FROM environments WHERE id = ?").run(id));
    });
  }

  mcpServers(ownerId: string, id: string): McpServer[] {
    this.row(ownerId, id);
    return this.mcpRows(id).map((server) => ({
      name: server.name,
      ...(JSON.parse(server.definition) as McpServerDefinition),
      secrets: Object.keys(this.mcpSecrets(server)),
    }));
  }

  /**
   * Creates or replaces an MCP server, its secrets included: a changed URL or
   * command never inherits the secrets assigned for the old one. The container
   * is recreated with the new configuration.
   */
  async putMcpServer(
    ownerId: string,
    id: string,
    name: string,
    body: Record<string, unknown>,
  ): Promise<McpServerResponse> {
    if (!mcpNamePattern.test(name)) {
      throw invalid("MCP server names are 1-63 lowercase letters, digits, '_' or '-'");
    }
    const { definition, secrets } = parseMcpServer(body);
    await this.reconfigure(ownerId, id, () => {
      const encrypted =
        Object.keys(secrets).length === 0
          ? null
          : this.secrets.encrypt(JSON.stringify(secrets), mcpSecretsContext(id, name));
      this.db
        .query(
          "INSERT OR REPLACE INTO mcp_servers (environment_id, name, definition, secrets) VALUES (?, ?, ?, ?)",
        )
        .run(id, name, JSON.stringify(definition), encrypted);
    });
    const server = { name, ...definition, secrets: Object.keys(secrets) };
    return server.secrets.length === 0 ? { server } : { server, warning: mcpSecretWarning };
  }

  async deleteMcpServer(ownerId: string, id: string, name: string): Promise<void> {
    this.row(ownerId, id);
    const server = "FROM mcp_servers WHERE environment_id = ? AND name = ?";
    if (this.db.query(`SELECT 1 ${server}`).get(id, name) === null) {
      throw new HttpError(404, "not_found", "no such MCP server");
    }
    await this.reconfigure(ownerId, id, () => this.db.query(`DELETE ${server}`).run(id, name));
  }

  /**
   * Replaces the container (the configuration is part of its environment)
   * with one built after `change`, and starts it if it was running. The old
   * container goes first: while it runs with the old configuration, the change
   * is not applied, so a failed removal changes nothing. A failure after it is
   * gone leaves a failed environment. The workspace volume stays.
   */
  private async reconfigure(ownerId: string, id: string, change: () => void): Promise<void> {
    await this.exclusive(ownerId, id, async (row) => {
      if (row.state !== "running" && row.state !== "stopped") throw notNow(row.state, "change");
      this.disconnect(id);
      await this.docker.removeContainer(containerName(id));
      try {
        change();
        this.db.query("UPDATE environments SET container_id = NULL WHERE id = ?").run(id);
        const containerId = await this.docker.createContainer(
          containerName(id),
          this.containerSpec(row),
        );
        this.db.query("UPDATE environments SET container_id = ? WHERE id = ?").run(containerId, id);
        if (row.state === "running") await this.docker.startContainer(containerId);
        this.setState(id, row.state);
      } catch (error) {
        this.setState(id, "failed");
        throw error;
      }
    });
  }

  private row(ownerId: string, id: string): EnvironmentRow {
    const row = this.db
      .query("SELECT * FROM environments WHERE id = ? AND owner_id = ?")
      .get(id, ownerId) as EnvironmentRow | null;
    if (row === null) throw new HttpError(404, "not_found", "no such environment");
    return row;
  }

  // Docker is asked even when the record already shows the target state: the
  // container may have exited on its own (crash, OOM kill, host restart), and
  // Docker treats starting a running or stopping a stopped container as a no-op.
  private async switchTo(
    ownerId: string,
    id: string,
    target: "running" | "stopped",
  ): Promise<Environment> {
    await this.exclusive(ownerId, id, async (row) => {
      const action = target === "running" ? "start" : "stop";
      if ((row.state !== "running" && row.state !== "stopped") || row.container_id === null) {
        throw notNow(row.state, action);
      }
      if (target === "stopped") this.disconnect(id);
      await (target === "running"
        ? this.docker.startContainer(row.container_id)
        : this.docker.stopContainer(row.container_id));
      this.setState(id, target);
    });
    return this.get(ownerId, id);
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

  /** Closes the environment's connections. */
  private disconnect(id: string): void {
    const open = this.connections.get(id);
    this.connections.delete(id);
    for (const connection of open ?? []) connection.close();
  }

  /** After a failed creation: removes the record only once Docker holds nothing for it. */
  private async discard(row: EnvironmentRow): Promise<void> {
    try {
      await this.docker.removeContainer(containerName(row.id));
      await this.docker.removeVolume(row.workspace_volume);
      await this.docker.removeVolume(homeVolume(row.id));
      this.broker.commit(() => this.db.query("DELETE FROM environments WHERE id = ?").run(row.id));
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

  private mcpRows(id: string): McpServerRow[] {
    return this.db
      .query("SELECT * FROM mcp_servers WHERE environment_id = ? ORDER BY name")
      .all(id) as McpServerRow[];
  }

  private mcpSecrets(server: McpServerRow): Record<string, string> {
    if (server.secrets === null) return {};
    const context = mcpSecretsContext(server.environment_id, server.name);
    return JSON.parse(this.secrets.decrypt(server.secrets, context)) as Record<string, string>;
  }

  private containerSpec(row: EnvironmentRow): ContainerSpec {
    const { memoryMiB, cpus, pids } = this.config.limits;
    const memory = memoryMiB * 1024 * 1024;
    const mcp = Object.fromEntries(
      this.mcpRows(row.id).map((server) => [
        server.name,
        opencodeMcp(JSON.parse(server.definition), this.mcpSecrets(server)),
      ]),
    );
    const key = this.db.query("SELECT provider FROM model_keys WHERE id = ?").get(row.key_id) as {
      provider: Provider;
    } | null;
    const env = [
      `OPENCODE_SERVER_PASSWORD=${this.secrets.decrypt(row.opencode_password, passwordContext(row.id))}`,
      `OPENCODE_CONFIG_CONTENT=${opencodeConfig(key?.provider ?? null, JSON.parse(row.models), mcp)}`,
    ];
    if (key !== null && row.broker_token !== null) {
      const token = this.secrets.decrypt(row.broker_token, brokerTokenContext(row.id));
      env.push(
        `TENTAFLAKE_MODEL_BASE_URL=${this.config.brokerUrl}`,
        `TENTAFLAKE_MODEL_API_KEY=${token}`,
      );
    }
    return {
      Image: this.config.image,
      User: "1000:1000",
      Env: env,
      Labels: { [environmentLabel]: row.id },
      HostConfig: {
        Runtime: "runsc",
        NetworkMode: networks[row.network_mode].name,
        CapDrop: ["ALL"],
        SecurityOpt: ["no-new-privileges"],
        ReadonlyRootfs: true,
        Tmpfs: { "/tmp": "exec,mode=1777" },
        Mounts: [
          { Type: "volume", Source: row.workspace_volume, Target: "/workspace" },
          { Type: "volume", Source: homeVolume(row.id), Target: "/home/agent" },
        ],
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
