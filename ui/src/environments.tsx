import { useEffect, useRef, useState } from "react";
import {
  api,
  type Environment,
  errorMessage,
  type McpServerRequest,
  type ModelKey,
  type NetworkMode,
} from "./client";
import { Confirm, Control, Empty, Form, Icon, Modal, Notice, StateBadge, useTask } from "./ui";

interface McpDraft {
  id: number;
  name: string;
  type: string;
  /** The command (local) or the URL (remote). */
  target: string;
  /** NAME=value lines: environment variables (local) or headers (remote). */
  secrets: string;
}

/** The request for a draft; throws when its secrets are malformed. */
function mcpRequest({ name, type, target, secrets: lines }: McpDraft): McpServerRequest {
  const secrets: Record<string, string> = {};
  for (const line of lines.split("\n").filter((line) => line.trim() !== "")) {
    const split = line.indexOf("=");
    if (split < 1) throw new Error(`${name}: secrets are NAME=value, one per line`);
    secrets[line.slice(0, split).trim()] = line.slice(split + 1);
  }
  const assigned = Object.keys(secrets).length > 0 ? { secrets } : {};
  return type === "local"
    ? { type, command: target.trim().split(/\s+/), ...assigned }
    : { type: "remote", url: target.trim(), ...assigned };
}

/** One MCP server of the environment being created. */
function McpRow({
  server,
  onChange,
  onRemove,
}: {
  server: McpDraft;
  onChange(patch: Partial<McpDraft>): void;
  onRemove(): void;
}) {
  const local = server.type === "local";
  return (
    <fieldset className="mcp">
      <div className="row">
        <Control
          spec={{ name: "name", label: "Server name", input: { pattern: "[A-Za-z0-9_\\-]{1,64}" } }}
          value={server.name}
          onChange={(name) => onChange({ name })}
        />
        <Control
          spec={{
            name: "type",
            label: "Type",
            options: [
              ["local", "local (runs in the environment)"],
              ["remote", "remote (reached by URL)"],
            ],
          }}
          value={server.type}
          onChange={(type) => onChange({ type })}
        />
      </div>
      <Control
        spec={{
          name: "target",
          label: local ? "Command" : "URL",
          hint: local ? "The program and its arguments, separated by spaces." : undefined,
        }}
        value={server.target}
        onChange={(target) => onChange({ target })}
      />
      <Control
        spec={{
          name: "secrets",
          label: local ? "Secret environment variables" : "Secret headers",
          hint: "NAME=value, one per line. Stored encrypted, but the agent in the environment can read them.",
          multiline: true,
          input: { required: false },
        }}
        value={server.secrets}
        onChange={(secrets) => onChange({ secrets })}
      />
      <button type="button" className="btn subtle danger" onClick={onRemove}>
        <Icon name="trash" />
        Remove server
      </button>
    </fieldset>
  );
}

function CreateEnvironment({
  keys,
  onCreated,
  onClose,
}: {
  keys: ModelKey[];
  onCreated(): void;
  onClose(): void;
}) {
  const [servers, setServers] = useState<McpDraft[]>([]);
  const [notes, setNotes] = useState<string[] | null>(null);
  const nextId = useRef(0);
  const change = (id: number, patch: Partial<McpDraft>) =>
    setServers(servers.map((server) => (server.id === id ? { ...server, ...patch } : server)));

  const create = async ({ name = "", networkMode = "", keyId = "", models = "" }) => {
    const requests = servers.map((server) => [server.name, mcpRequest(server)] as const);
    const created = await api.createEnvironment({
      name,
      networkMode: networkMode as NetworkMode,
      keyId,
      models: models.split(/[\s,]+/).filter(Boolean),
    });
    const found: string[] = [];
    try {
      // Each change replaces the container, so the servers go in one after the other.
      for (const [serverName, definition] of requests) {
        const { warning } = await api.putMcpServer(created.id, serverName, definition);
        if (warning) found.push(`${serverName}: ${warning}`);
      }
    } catch (error) {
      found.push(
        `The environment exists, but adding its MCP servers stopped: ${errorMessage(error)}`,
      );
    }
    onCreated();
    if (found.length === 0) onClose();
    else setNotes(found);
  };

  return (
    <Modal title="Create an environment" onClose={onClose} wide>
      {notes ? (
        <>
          <div className="dialog-body">
            <Notice tone="ok">The environment was created.</Notice>
            {notes.map((note) => (
              <Notice key={note} tone="warn">
                {note}
              </Notice>
            ))}
          </div>
          <div className="dialog-foot">
            <button type="button" className="btn primary" onClick={onClose}>
              Done
            </button>
          </div>
        </>
      ) : keys.length === 0 ? (
        <div className="dialog-body">
          <Notice tone="warn">
            Add a model key first: <a href="#/keys">Model keys</a>.
          </Notice>
        </div>
      ) : (
        <Form
          onCancel={onClose}
          action="Create environment"
          icon="plus"
          initial={{ networkMode: "model-only", keyId: keys[0]?.id ?? "" }}
          onSubmit={create}
          fields={[
            {
              name: "name",
              label: "Name",
              input: { maxLength: 64, placeholder: "e.g. research" },
            },
            {
              name: "networkMode",
              label: "Network",
              options: [
                ["model-only", "Model only: reaches the model broker and nothing else"],
                ["open", "Open internet: the agent can reach any address"],
              ],
            },
            {
              name: "keyId",
              label: "Key",
              options: keys.map((key) => [key.id, `${key.label} (${key.provider})`]),
            },
            {
              name: "models",
              label: "Models",
              hint: "Model names as the provider knows them, separated by commas. The first is the default.",
            },
          ]}
        >
          <section className="mcp-list">
            <h3>MCP servers</h3>
            {servers.map((server) => (
              <McpRow
                key={server.id}
                server={server}
                onChange={(patch) => change(server.id, patch)}
                onRemove={() => setServers(servers.filter((other) => other.id !== server.id))}
              />
            ))}
            <button
              type="button"
              className="btn"
              onClick={() =>
                setServers([
                  ...servers,
                  { id: nextId.current++, name: "", type: "local", target: "", secrets: "" },
                ])
              }
            >
              <Icon name="plus" />
              Add MCP server
            </button>
          </section>
        </Form>
      )}
    </Modal>
  );
}

/** The caller's environments with their lifecycle actions. */
export function Environments() {
  const [list, setList] = useState<Environment[] | null>(null);
  const [keys, setKeys] = useState<ModelKey[]>([]);
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<Environment | null>(null);
  const task = useTask();
  const load = () =>
    task.run(async () => {
      const [environments, models] = await Promise.all([api.environments(), api.keys()]);
      setList(environments);
      setKeys(models);
    });
  // biome-ignore lint/correctness/useExhaustiveDependencies: load once, then refresh quietly (idle stops happen on the host)
  useEffect(() => {
    void load();
    const timer = setInterval(() => api.environments().then(setList, () => {}), 10_000);
    return () => clearInterval(timer);
  }, []);
  const lifecycle = (action: (id: string) => Promise<Environment>, env: Environment) =>
    task.run(async () => {
      const changed = await action(env.id);
      setList(
        (current) => current?.map((item) => (item.id === changed.id ? changed : item)) ?? null,
      );
    });

  return (
    <div className="page">
      <div className="row">
        <h1>Environments</h1>
        <span className="spacer" />
        <button type="button" className="btn primary" onClick={() => setCreating(true)}>
          <Icon name="plus" />
          Create environment
        </button>
      </div>
      {task.error && <Notice tone="bad">{task.error}</Notice>}
      {list?.length === 0 && (
        <Empty icon="box" title="No environments yet">
          <p>
            An environment is an isolated container with an agent. Add a model key, then create one.
          </p>
        </Empty>
      )}
      {list && list.length > 0 && (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>State</th>
                <th>Network</th>
                <th>Key</th>
                <th>Models</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {list.map((env) => (
                <tr key={env.id}>
                  <td>
                    {env.state === "running" ? (
                      <a href={`#/environments/${env.id}`}>
                        <strong>{env.name}</strong>
                      </a>
                    ) : (
                      <strong>{env.name}</strong>
                    )}
                  </td>
                  <td>
                    <StateBadge state={env.state} />
                  </td>
                  <td>
                    <span className={env.networkMode === "open" ? "chip warn" : "chip"}>
                      {env.networkMode === "open" ? "open internet" : "model only"}
                    </span>
                  </td>
                  <td>{keys.find((key) => key.id === env.keyId)?.label ?? "deleted"}</td>
                  <td className="muted">{env.models.join(", ")}</td>
                  <td className="actions">
                    {env.state === "running" && (
                      <>
                        <a className="btn primary" href={`#/environments/${env.id}`}>
                          <Icon name="chat" />
                          Open
                        </a>
                        <button
                          type="button"
                          className="btn"
                          disabled={task.busy}
                          onClick={() => lifecycle(api.stopEnvironment, env)}
                        >
                          <Icon name="stop" />
                          Stop
                        </button>
                      </>
                    )}
                    {env.state === "stopped" && (
                      <button
                        type="button"
                        className="btn"
                        disabled={task.busy}
                        onClick={() => lifecycle(api.startEnvironment, env)}
                      >
                        <Icon name="play" />
                        Start
                      </button>
                    )}
                    <button type="button" className="btn danger" onClick={() => setDeleting(env)}>
                      <Icon name="trash" />
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {creating && (
        <CreateEnvironment
          keys={keys}
          onCreated={() => void load()}
          onClose={() => setCreating(false)}
        />
      )}
      {deleting && (
        <Confirm
          title={`Delete ${deleting.name}?`}
          action="Delete environment"
          onClose={() => setDeleting(null)}
          onConfirm={async () => {
            await api.deleteEnvironment(deleting.id);
            setList((current) => current?.filter((env) => env.id !== deleting.id) ?? null);
          }}
        >
          <p>
            This removes the container, its workspace and every file in it. It cannot be undone.
          </p>
        </Confirm>
      )}
    </div>
  );
}
