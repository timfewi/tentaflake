import { type ReactNode, useEffect, useState } from "react";
import { Chat } from "./chat";
import { api, type Environment, errorMessage } from "./client";
import { Files } from "./files";
import type { IconName } from "./icons";
import { TerminalView } from "./terminal";
import { Empty, Icon, Notice, StateBadge, useTask } from "./ui";

const tabs: { name: string; icon: IconName }[] = [
  { name: "Chat", icon: "chat" },
  { name: "Files", icon: "folder" },
  { name: "Terminal", icon: "terminal" },
];

/** One environment: chat with approvals, its workspace files and a terminal. */
export function EnvironmentPage({ id }: { id: string }) {
  const [env, setEnv] = useState<Environment | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [tab, setTab] = useState("Chat");
  const [visited, setVisited] = useState(["Chat"]);
  const task = useTask();
  const refresh = () =>
    api.environment(id).then(
      (found) => {
        setEnv(found);
        setFailure(null);
      },
      (error) => setFailure(errorMessage(error)),
    );
  // biome-ignore lint/correctness/useExhaustiveDependencies: reload per environment; the host can stop it when idle
  useEffect(() => {
    void refresh();
    const timer = setInterval(refresh, 10_000);
    return () => clearInterval(timer);
  }, [id]);

  if (env === null) {
    return (
      <div className="page">
        {failure ? (
          <Notice tone="bad">
            {failure} <a href="#/environments">Back to the environments</a>
          </Notice>
        ) : (
          <p className="muted">Loading…</p>
        )}
      </div>
    );
  }
  const running = env.state === "running";
  // A panel stays mounted once opened, so the event stream and the shell survive a tab change.
  const panel = (name: string, content: ReactNode) =>
    visited.includes(name) && (
      <div role="tabpanel" hidden={tab !== name}>
        {content}
      </div>
    );
  return (
    <div className="page">
      <div className="row">
        <a className="crumb" href="#/environments">
          Environments
        </a>
        <Icon name="chevron" className="faint" />
        <h1>{env.name}</h1>
        <StateBadge state={env.state} />
        <span className="chip">{env.networkMode === "open" ? "open internet" : "model only"}</span>
        <span className="spacer" />
        <button
          type="button"
          className="btn"
          disabled={task.busy || (!running && env.state !== "stopped")}
          onClick={() =>
            task.run(async () =>
              setEnv(await (running ? api.stopEnvironment(id) : api.startEnvironment(id))),
            )
          }
        >
          <Icon name={running ? "stop" : "play"} />
          {running ? "Stop" : "Start"}
        </button>
      </div>
      {(task.error ?? failure) && <Notice tone="bad">{task.error ?? failure}</Notice>}
      {running && (
        <div className="tabs" role="tablist">
          {tabs.map(({ name, icon }) => (
            <button
              key={name}
              type="button"
              role="tab"
              aria-selected={tab === name}
              onClick={() => {
                setTab(name);
                if (!visited.includes(name)) setVisited([...visited, name]);
              }}
            >
              <Icon name={icon} />
              {name}
            </button>
          ))}
        </div>
      )}
      {running ? (
        <>
          {panel("Chat", <Chat env={env} onStopped={() => void refresh()} />)}
          {panel("Files", <Files envId={id} />)}
          {panel(
            "Terminal",
            <TerminalView name={env.name} active={tab === "Terminal"} envId={id} />,
          )}
        </>
      ) : (
        <Empty icon="box" title={`The environment is ${env.state}`}>
          <p>
            {env.state === "stopped"
              ? "Start it to chat, browse its files or open a terminal. Its workspace and sessions are kept."
              : "It cannot be started. If it stays that way, delete it in the environment list."}
          </p>
        </Empty>
      )}
    </div>
  );
}
