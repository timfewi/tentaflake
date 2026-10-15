import type { AgentEvent, AgentSession, Permission } from "./client";

/** One entry of a conversation; tool calls fill `name`, `input` and `output` or `error` as events arrive. */
export interface Item {
  kind: "user" | "assistant" | "tool" | "notice";
  id: string;
  text: string;
  name?: string;
  input?: Record<string, unknown>;
  output?: string;
  error?: string;
}

export interface ChatState {
  sessions: AgentSession[];
  /** Tool calls waiting for the owner's reply, in every session. */
  permissions: Permission[];
  /** What this page saw per session: the service replays nothing. */
  items: Record<string, Item[]>;
  /** Events that arrived while a snapshot loads; null when none is loading. */
  held: AgentEvent[] | null;
}

export const emptyChat: ChatState = { sessions: [], permissions: [], items: {}, held: null };

export type ChatAction =
  /** A snapshot of sessions and approvals is being loaded: hold the events that arrive meanwhile. */
  | { type: "syncing" }
  /** The snapshot arrived (or failed: none), and the held events apply on top of it. */
  | { type: "sync"; snapshot?: { sessions: AgentSession[]; permissions: Permission[] } }
  | { type: "created"; session: AgentSession }
  | { type: "prompted"; sessionId: string; messageId: string; text: string }
  | { type: "answered"; permissionId: string }
  | { type: "event"; event: AgentEvent };

const append = (state: ChatState, sessionId: string, item: Item): ChatState => ({
  ...state,
  items: { ...state.items, [sessionId]: [...(state.items[sessionId] ?? []), item] },
});

/** Changes the item `id` of a session, first adding it when the page did not see it begin. */
function change(
  state: ChatState,
  sessionId: string,
  kind: Item["kind"],
  id: string,
  update: (item: Item) => Partial<Item>,
): ChatState {
  const list = state.items[sessionId] ?? [];
  const known = list.some((item) => item.kind === kind && item.id === id);
  const next = (known ? list : [...list, { kind, id, text: "" }]).map((item) =>
    item.kind === kind && item.id === id ? { ...item, ...update(item) } : item,
  );
  return { ...state, items: { ...state.items, [sessionId]: next } };
}

const patch = (state: ChatState, id: string, update: Partial<AgentSession>): ChatState => ({
  ...state,
  sessions: state.sessions.map((session) =>
    session.id === id ? { ...session, ...update } : session,
  ),
});

/** A prompt is over; a note says how when it did not succeed. */
function finish(state: ChatState, sessionId: string, note?: string): ChatState {
  const next = patch(state, sessionId, { running: false });
  const id = `n${next.items[sessionId]?.length ?? 0}`;
  return note === undefined ? next : append(next, sessionId, { kind: "notice", id, text: note });
}

function reduceEvent(state: ChatState, event: AgentEvent): ChatState {
  switch (event.type) {
    case "session.created": {
      const now = new Date().toISOString();
      const session = {
        id: event.sessionId,
        title: null,
        createdAt: now,
        updatedAt: now,
        running: false,
      };
      return reduceChat(state, { type: "created", session });
    }
    case "session.renamed":
      return patch(state, event.sessionId, { title: event.title });
    case "session.deleted":
      return {
        ...state,
        sessions: state.sessions.filter((session) => session.id !== event.sessionId),
      };
    case "session.execution.started":
      return patch(state, event.sessionId, { running: true });
    case "session.execution.succeeded":
      return finish(state, event.sessionId);
    case "session.execution.interrupted":
      return finish(state, event.sessionId, "Stopped.");
    case "session.execution.failed":
      return finish(state, event.sessionId, `The prompt failed: ${event.error}`);
    case "session.text.delta":
      return change(state, event.sessionId, "assistant", event.messageId, (item) => ({
        text: item.text + event.delta,
      }));
    case "session.text.ended":
      // The final text wins unless the deltas already built a longer one (a message with several parts).
      return change(state, event.sessionId, "assistant", event.messageId, (item) =>
        event.text.length >= item.text.length ? { text: event.text } : {},
      );
    case "session.tool.input.started":
      return change(state, event.sessionId, "tool", event.callId, () => ({ name: event.name }));
    case "session.tool.called":
      return change(state, event.sessionId, "tool", event.callId, () => ({ input: event.input }));
    case "session.tool.success":
      return change(state, event.sessionId, "tool", event.callId, () => ({ output: event.output }));
    case "session.tool.failed":
      return change(state, event.sessionId, "tool", event.callId, () => ({ error: event.error }));
    case "permission.asked": {
      const { id, sessionId, action, resources } = event;
      return state.permissions.some((permission) => permission.id === id)
        ? state
        : { ...state, permissions: [...state.permissions, { id, sessionId, action, resources }] };
    }
    case "permission.replied":
      return reduceChat(state, { type: "answered", permissionId: event.permissionId });
    case "environment.stopped":
      return state;
  }
}

export function reduceChat(state: ChatState, action: ChatAction): ChatState {
  switch (action.type) {
    case "syncing":
      return { ...state, held: [] };
    case "sync": {
      const { sessions, permissions } = action.snapshot ?? state;
      const base = { ...state, sessions, permissions, held: null };
      return (state.held ?? []).reduce(reduceEvent, base);
    }
    case "created":
      return state.sessions.some((session) => session.id === action.session.id)
        ? state
        : { ...state, sessions: [action.session, ...state.sessions] };
    case "prompted":
      return append(state, action.sessionId, {
        kind: "user",
        id: action.messageId,
        text: action.text,
      });
    case "answered":
      return {
        ...state,
        permissions: state.permissions.filter(({ id }) => id !== action.permissionId),
      };
    case "event":
      return state.held === null
        ? reduceEvent(state, action.event)
        : { ...state, held: [...state.held, action.event] };
  }
}
