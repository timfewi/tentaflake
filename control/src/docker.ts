// The Docker Engine API calls the service makes, over the daemon's Unix
// socket. Tests substitute a fake that implements the same interface.

/** A Docker Engine request that failed; `status` is the HTTP status, 0 if Docker did not answer. */
export class DockerError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

/** The body of POST /containers/create (only the fields the service sets). */
export interface ContainerSpec {
  Image: string;
  User: string;
  Env: string[];
  Labels: Record<string, string>;
  HostConfig: {
    Runtime: string;
    NetworkMode: string;
    CapDrop: string[];
    SecurityOpt: string[];
    ReadonlyRootfs: boolean;
    Tmpfs: Record<string, string>;
    Mounts: { Type: "volume"; Source: string; Target: string }[];
    Memory: number;
    MemorySwap: number;
    NanoCpus: number;
    PidsLimit: number;
  };
}

export interface Docker {
  /** The network's `internal` flag, IPv4 gateway and bridge name, or null if it does not exist. */
  network(
    name: string,
  ): Promise<{ internal: boolean; gateway: string | null; bridge: string | null } | null>;
  createNetwork(name: string, internal: boolean, bridge: string): Promise<void>;
  createVolume(name: string, labels: Record<string, string>): Promise<void>;
  /** Succeeds if the volume is already gone. */
  removeVolume(name: string): Promise<void>;
  /** Returns the container id. */
  createContainer(name: string, spec: ContainerSpec): Promise<string>;
  /** Succeeds if the container is already running. */
  startContainer(id: string): Promise<void>;
  /** Succeeds if the container is already stopped. */
  stopContainer(id: string): Promise<void>;
  /** Force-removes a container by id or name; succeeds if it is already gone. */
  removeContainer(id: string): Promise<void>;
  /** The container's IPv4 address on the network, or null if it is not running there. */
  containerAddress(id: string, network: string): Promise<string | null>;
}

// The oldest API version current Docker Engine releases still accept.
const apiVersion = "v1.44";
const bridgeOption = "com.docker.network.bridge.name";

export function dockerClient(socket: string): Docker {
  async function call(method: string, path: string, body?: unknown, accept: number[] = []) {
    let response: Response;
    try {
      response = await fetch(`http://docker/${apiVersion}${path}`, {
        method,
        unix: socket,
        ...(body === undefined
          ? {}
          : { headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
      });
    } catch (error) {
      throw new DockerError(0, `Docker ${method} ${path}: ${(error as Error).message}`);
    }
    if (!response.ok && !accept.includes(response.status)) {
      const text = await response.text();
      let detail = text;
      try {
        detail = JSON.parse(text).message ?? text;
      } catch {}
      throw new DockerError(
        response.status,
        `Docker ${method} ${path}: ${response.status} ${detail}`,
      );
    }
    return response;
  }
  const segment = encodeURIComponent;

  return {
    async network(name) {
      const response = await call("GET", `/networks/${segment(name)}`, undefined, [404]);
      if (response.status === 404) return null;
      const network = (await response.json()) as {
        Internal?: boolean;
        IPAM?: { Config?: { Gateway?: string }[] | null };
        Options?: Record<string, string> | null;
      };
      const gateway = network.IPAM?.Config?.find((config) => config.Gateway?.includes("."));
      return {
        internal: network.Internal === true,
        gateway: gateway?.Gateway ?? null,
        bridge: network.Options?.[bridgeOption] ?? null,
      };
    },
    async createNetwork(name, internal, bridge) {
      await call("POST", "/networks/create", {
        Name: name,
        Driver: "bridge",
        Internal: internal,
        Options: { [bridgeOption]: bridge },
      });
    },
    async createVolume(name, labels) {
      await call("POST", "/volumes/create", { Name: name, Labels: labels });
    },
    async removeVolume(name) {
      await call("DELETE", `/volumes/${segment(name)}`, undefined, [404]);
    },
    async createContainer(name, spec) {
      const response = await call("POST", `/containers/create?name=${segment(name)}`, spec);
      return ((await response.json()) as { Id: string }).Id;
    },
    async startContainer(id) {
      await call("POST", `/containers/${segment(id)}/start`, undefined, [304]);
    },
    async stopContainer(id) {
      await call("POST", `/containers/${segment(id)}/stop`, undefined, [304]);
    },
    async removeContainer(id) {
      await call("DELETE", `/containers/${segment(id)}?force=true`, undefined, [404]);
    },
    async containerAddress(id, network) {
      const response = await call("GET", `/containers/${segment(id)}/json`, undefined, [404]);
      if (response.status === 404) return null;
      const container = (await response.json()) as {
        State?: { Running?: boolean };
        NetworkSettings?: { Networks?: Record<string, { IPAddress?: string }> };
      };
      const address = container.NetworkSettings?.Networks?.[network]?.IPAddress;
      return container.State?.Running === true && address ? address : null;
    },
  };
}
