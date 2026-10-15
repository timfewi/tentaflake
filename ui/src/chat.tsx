import { useEffect, useReducer, useRef, useState } from "react";
import { emptyChat, type Item, reduceChat } from "./chat-state";
import { type AgentEvent, api, type Environment, eventsUrl, type PermissionReply } from "./client";
import { SafeMarkdown } from "./markdown";
import { Notice, useTask } from "./ui";

/** Agent output is untrusted: text and Markdown only, never HTML. */
function Entry({ item }: { item: Item }) {
  if (item.kind === "assistant") return <SafeMarkdown body={item.text} />;
  if (item.kind !== "tool") return <p className={`msg ${item.kind}`}>{item.text}</p>;
  const status =
    item.error !== undefined ? "failed" : item.output !== undefined ? "done" : "running";
  return (
    <details open={item.error !== undefined}>
      <summary>
        {item.name ?? "tool"} <span className={`chip ${status}`}>{status}</span>
      </summary>
      {item.input && <pre>{JSON.stringify(item.input, null, 2)}</pre>}
      {item.output !== undefined && <pre>{item.output.slice(0, 20_000)}</pre>}
      {item.error !== undefined && <pre className="failed">{item.error}</pre>}
    </details>
  );
}

const replies: [PermissionReply, string][] = [
  ["once", "Allow once"],
  ["always", "Always allow"],
  ["reject", "Reject"],
];

/** Sessions and chat of a running environment, live from its event stream. */
export function Chat({ env, onStopped }: { env: Environment; onStopped(): void }) {
  const [state, dispatch] = useReducer(reduceChat, emptyChat);
  const [selected, setSelected] = useState<string | null>(null);
  const [live, setLive] = useState(false);
  const [text, setText] = useState("");
  const task = useTask();
  const end = useRef<HTMLDivElement>(null);
  const current = state.sessions.find((session) => session.id === selected) ?? state.sessions[0];
  const items = current ? (state.items[current.id] ?? []) : [];

  // biome-ignore lint/correctness/useExhaustiveDependencies: one stream per environment; the callbacks only dispatch
  useEffect(() => {
    const source = new EventSource(eventsUrl(env.id));
    // Nothing is replayed, so every (re)connect starts from the current sessions and approvals;
    // events that arrive while they load are held and applied on top of them.
    source.onopen = () => {
      setLive(true);
      dispatch({ type: "syncing" });
      void task
        .run(async () => {
          const [sessions, permissions] = await Promise.all([
            api.sessions(env.id),
            api.permissions(env.id),
          ]);
          dispatch({ type: "sync", snapshot: { sessions, permissions } });
        })
        .then((ok) => ok || dispatch({ type: "sync" }));
    };
    source.onerror = () => setLive(false);
    source.onmessage = (message) => {
      try {
        const event = JSON.parse(message.data) as AgentEvent;
        if (event.type !== "environment.stopped") return dispatch({ type: "event", event });
        source.close();
        onStopped();
      } catch {}
    };
    return () => source.close();
  }, [env.id]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll when the transcript changes
  useEffect(() => end.current?.scrollIntoView({ block: "end" }), [items]);

  const newSession = async () => {
    const created = await api.createSession(env.id);
    dispatch({ type: "created", session: created });
    setSelected(created.id);
    return created.id;
  };
  const send = async () => {
    const prompt = text.trim();
    if (prompt === "") return;
    const ok = await task.run(async () => {
      const id = current?.id ?? (await newSession());
      const { messageId } = await api.prompt(env.id, id, prompt);
      dispatch({ type: "prompted", sessionId: id, messageId, text: prompt });
    });
    if (ok) setText("");
  };

  return (
    <div className="chat">
      <aside>
        <button
          type="button"
          className="btn"
          disabled={task.busy}
          onClick={() => task.run(newSession)}
        >
          New session
        </button>
        <ul aria-label="Sessions">
          {state.sessions.map((session) => (
            <li key={session.id}>
              <button
                type="button"
                aria-current={session.id === current?.id}
                onClick={() => setSelected(session.id)}
              >
                {session.title ?? `Session ${session.id.slice(-6)}`}
                {session.running && <span className="chip running">running</span>}
              </button>
            </li>
          ))}
        </ul>
        <span className={`chip ${live ? "done" : "running"}`}>
          {live ? "Live" : "Reconnecting…"}
        </span>
      </aside>
      <section>
        {state.permissions.map(({ id, sessionId, action, resources }) => (
          <section className="approval" key={id} aria-label={`Approval: ${action}`}>
            <strong>Approval needed: {action}</strong>
            {resources.map((resource) => (
              <code key={resource}>{resource}</code>
            ))}
            <div className="row">
              {replies.map(([reply, label]) => (
                <button
                  key={reply}
                  type="button"
                  className={reply === "once" ? "btn primary" : "btn"}
                  onClick={() =>
                    task.run(async () => {
                      await api.reply(env.id, sessionId, id, reply);
                      dispatch({ type: "answered", permissionId: id });
                    })
                  }
                >
                  {label}
                </button>
              ))}
            </div>
          </section>
        ))}
        <div className="transcript" role="log" aria-label="Conversation">
          {items.length === 0 && (
            <p className="muted">
              Ask the agent for something. The service does not replay earlier messages.
            </p>
          )}
          {items.map((item) => (
            <Entry key={`${item.kind}-${item.id}`} item={item} />
          ))}
          <div ref={end} />
        </div>
        {task.error && <Notice tone="bad">{task.error}</Notice>}
        <form
          className="composer"
          onSubmit={(event) => {
            event.preventDefault();
            void send();
          }}
        >
          <textarea
            aria-label="Message"
            placeholder="Message the agent"
            value={text}
            maxLength={32768}
            rows={2}
            onChange={(event) => setText(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                void send();
              }
            }}
          />
          <button type="submit" className="btn primary" disabled={task.busy || text.trim() === ""}>
            Send
          </button>
          {current?.running && (
            <button
              type="button"
              className="btn"
              onClick={() => task.run(() => api.abort(env.id, current.id))}
            >
              Stop
            </button>
          )}
        </form>
      </section>
    </div>
  );
}
