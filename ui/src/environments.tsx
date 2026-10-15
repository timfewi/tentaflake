import { useEffect, useState } from "react";
import {
  api,
  type Environment,
  errorMessage,
  type McpServerRequest,
  type ModelKey,
  type NetworkMode,
} from "./client";
import { Confirm, Form, Modal, Notice, StateBadge, useTask } from "./ui";

const mcpExample =
  '{"docs": {"type": "remote", "url": "https://mcp.example/sse", "secrets": {"Authorization": "Bearer …"}}}';

function CreateEnvironment({
  keys,
  onCreated,
  onClose,
}: {
  keys: ModelKey[];
  onCreated(): void;
  onClose(): void;
}) {
  const [notes, setNotes] = useState<string[] | null>(null);

  const create = async ({ name = "", networkMode = "", keyId = "", models = "", mcp = "" }) => {
    const servers = mcp.trim() === "" ? {} : JSON.parse(mcp);
    if (typeof servers !== "object" || servers === null || Array.isArray(servers)) {
      throw new Error("MCP servers are a JSON object of name: definition");
    }
    const created = await api.createEnvironment({
      name,
      networkMode: networkMode as NetworkMode,
      keyId,
      models: models.split(/[\s,]+/).filter(Boolean),
    });
    const found: string[] = [];
    try {
      // Each change replaces the container, so the servers go in one after the other.
      for (const [serverName, definition] of Object.entries<McpServerRequest>(servers)) {
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
    <Modal title="Create an environment" onClose={onClose}>
      <div className="body">
        {notes ? (
          <>
            <Notice tone="ok">The environment was created.</Notice>
            {notes.map((note) => (
              <Notice key={note} tone="warn">
                {note}
              </Notice>
            ))}
          </>
        ) : keys.length === 0 ? (
          <Notice tone="warn">
            Add a model key first: <a href="#/keys">Model keys</a>.
          </Notice>
        ) : (
          <Form
            action="Create"
            initial={{ networkMode: "model-only", keyId: keys[0]?.id ?? "" }}
            onSubmit={create}
            fields={[
              { name: "name", label: "Name", input: { maxLength: 64 } },
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
              {
                name: "mcp",
                label: "MCP servers (JSON, optional)",
                multiline: true,
                hint: (
                  <>
                    One definition per name, e.g. <code>{mcpExample}</code> Values under "secrets"
                    are stored encrypted, but the agent in the environment can read them.
                  </>
                ),
              },
            ]}
          />
        )}
      </div>
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
          Create environment
        </button>
      </div>
      {task.error && <Notice tone="bad">{task.error}</Notice>}
      {list?.length === 0 && (
        <p className="muted">No environments yet. Add a model key, then create one.</p>
      )}
      {list && list.length > 0 && (
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
                    <a href={`#/environments/${env.id}`}>{env.name}</a>
                  ) : (
                    env.name
                  )}
                </td>
                <td>
                  <StateBadge state={env.state} />
                </td>
                <td>{env.networkMode === "open" ? "open internet" : "model only"}</td>
                <td>{keys.find((key) => key.id === env.keyId)?.label ?? "deleted"}</td>
                <td>{env.models.join(", ")}</td>
                <td className="actions">
                  {env.state === "running" ? (
                    <button
                      type="button"
                      className="btn"
                      disabled={task.busy}
                      onClick={() => lifecycle(api.stopEnvironment, env)}
                    >
                      Stop
                    </button>
                  ) : (
                    env.state !== "creating" && (
                      <button
                        type="button"
                        className="btn"
                        disabled={task.busy}
                        onClick={() => lifecycle(api.startEnvironment, env)}
                      >
                        Start
                      </button>
                    )
                  )}
                  <button type="button" className="btn danger" onClick={() => setDeleting(env)}>
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
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
