import { type ReactNode, useEffect, useState } from "react";
import { Chat } from "./chat";
import { api, type Environment, errorMessage } from "./client";
import { Files } from "./files";
import { TerminalView } from "./terminal";
import { Notice, StateBadge, useTask } from "./ui";

const tabs = ["Chat", "Files", "Terminal"] as const;
type Tab = (typeof tabs)[number];

/** One environment: chat with approvals, its workspace files and a terminal. */
export function EnvironmentPage({ id }: { id: string }) {
  const [env, setEnv] = useState<Environment | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("Chat");
  const [visited, setVisited] = useState<Tab[]>(["Chat"]);
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
  const panel = (name: Tab, content: ReactNode) =>
    visited.includes(name) && (
      <div role="tabpanel" hidden={tab !== name}>
        {content}
      </div>
    );
  return (
    <div className="page">
      <div className="row">
        <a href="#/environments">Environments</a>
        <h1>{env.name}</h1>
        <StateBadge state={env.state} />
        <span className="spacer" />
        <button
          type="button"
          className="btn"
          disabled={task.busy || env.state === "creating"}
          onClick={() =>
            task.run(async () =>
              setEnv(await (running ? api.stopEnvironment(id) : api.startEnvironment(id))),
            )
          }
        >
          {running ? "Stop" : "Start"}
        </button>
      </div>
      {(task.error ?? failure) && <Notice tone="bad">{task.error ?? failure}</Notice>}
      <div className="tabs" role="tablist">
        {tabs.map((name) => (
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
            {name}
          </button>
        ))}
      </div>
      {running ? (
        <>
          {panel("Chat", <Chat env={env} onStopped={() => void refresh()} />)}
          {panel("Files", <Files envId={id} />)}
          {panel("Terminal", <TerminalView envId={id} active={tab === "Terminal"} />)}
        </>
      ) : (
        <p className="muted">
          The environment is not running. Start it to chat, browse its files or open a terminal; its
          workspace and sessions are kept.
        </p>
      )}
    </div>
  );
}
