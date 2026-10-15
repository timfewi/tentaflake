import { expect, test } from "bun:test";
import { type ChatAction, type ChatState, emptyChat, reduceChat } from "./chat-state";
import type { AgentEvent, AgentSession } from "./client";

const session = (id: string, patch: Partial<AgentSession> = {}): AgentSession => ({
  id,
  title: null,
  createdAt: "",
  updatedAt: "",
  running: false,
  ...patch,
});
const play = (...actions: ChatAction[]) => actions.reduce(reduceChat, emptyChat);
const events = (...list: AgentEvent[]) =>
  play(...list.map((event) => ({ type: "event" as const, event })));
const s = "ses_1";

test("a reply streams into one message and the final text settles it", () => {
  const streaming = events(
    { type: "session.text.delta", sessionId: s, messageId: "m1", delta: "Hel" },
    { type: "session.text.delta", sessionId: s, messageId: "m1", delta: "lo" },
  );
  expect(streaming.items[s]).toEqual([{ kind: "assistant", id: "m1", text: "Hello" }]);
  const ended = reduceChat(streaming, {
    type: "event",
    event: { type: "session.text.ended", sessionId: s, messageId: "m1", text: "Hello there" },
  });
  expect(ended.items[s]).toEqual([{ kind: "assistant", id: "m1", text: "Hello there" }]);
});

test("a page that joined mid-message still shows the final text, and a part never shortens it", () => {
  const joined = events({
    type: "session.text.ended",
    sessionId: s,
    messageId: "m1",
    text: "All of it",
  });
  expect(joined.items[s]).toEqual([{ kind: "assistant", id: "m1", text: "All of it" }]);
  const parts = events(
    { type: "session.text.delta", sessionId: s, messageId: "m2", delta: "first part, second" },
    { type: "session.text.ended", sessionId: s, messageId: "m2", text: "second" },
  );
  expect(parts.items[s]).toEqual([{ kind: "assistant", id: "m2", text: "first part, second" }]);
});

test("a tool call collects its input, output or error in order with the text", () => {
  const state = events(
    { type: "session.text.delta", sessionId: s, messageId: "m1", delta: "Running" },
    {
      type: "session.tool.input.started",
      sessionId: s,
      messageId: "m1",
      callId: "c1",
      name: "shell",
    },
    {
      type: "session.tool.called",
      sessionId: s,
      messageId: "m1",
      callId: "c1",
      input: { command: "ls" },
    },
    { type: "session.tool.success", sessionId: s, messageId: "m1", callId: "c1", output: "a.txt" },
    {
      type: "session.tool.input.started",
      sessionId: s,
      messageId: "m1",
      callId: "c2",
      name: "read",
    },
    { type: "session.tool.failed", sessionId: s, messageId: "m1", callId: "c2", error: "no file" },
  );
  expect(state.items[s]).toEqual([
    { kind: "assistant", id: "m1", text: "Running" },
    { kind: "tool", id: "c1", text: "", name: "shell", input: { command: "ls" }, output: "a.txt" },
    { kind: "tool", id: "c2", text: "", name: "read", error: "no file" },
  ]);
});

test("approvals wait until they are replied to, by event or by the owner, and are not doubled", () => {
  const asked: AgentEvent = {
    type: "permission.asked",
    id: "p1",
    sessionId: s,
    action: "shell",
    resources: ["ls"],
  };
  const waiting = events(asked, asked, { ...asked, id: "p2" });
  expect(waiting.permissions.map((permission) => permission.id)).toEqual(["p1", "p2"]);
  const replied = reduceChat(waiting, {
    type: "event",
    event: { type: "permission.replied", sessionId: s, permissionId: "p1", reply: "once" },
  });
  expect(replied.permissions.map((permission) => permission.id)).toEqual(["p2"]);
  expect(reduceChat(replied, { type: "answered", permissionId: "p2" }).permissions).toEqual([]);
});

test("sessions follow created, renamed, running and deleted events", () => {
  let state = events(
    { type: "session.created", sessionId: s },
    { type: "session.created", sessionId: s },
  );
  expect(state.sessions).toHaveLength(1);
  state = reduceChat(state, {
    type: "event",
    event: { type: "session.renamed", sessionId: s, title: "Hello" },
  });
  state = reduceChat(state, {
    type: "event",
    event: { type: "session.execution.started", sessionId: s },
  });
  expect(state.sessions[0]).toMatchObject({ title: "Hello", running: true });
  state = reduceChat(state, {
    type: "event",
    event: { type: "session.execution.succeeded", sessionId: s },
  });
  expect(state.sessions[0]?.running).toBe(false);
  state = reduceChat(state, { type: "event", event: { type: "session.deleted", sessionId: s } });
  expect(state.sessions).toEqual([]);
});

test("a failed or stopped prompt leaves a note and ends the running state", () => {
  const state = events(
    { type: "session.created", sessionId: s },
    { type: "session.execution.started", sessionId: s },
    { type: "session.execution.failed", sessionId: s, error: "402" },
    { type: "session.execution.started", sessionId: s },
    { type: "session.execution.interrupted", sessionId: s },
  );
  expect(state.sessions[0]?.running).toBe(false);
  expect(state.items[s]?.map((item) => item.kind === "notice" && item.text)).toEqual([
    "The prompt failed: 402",
    "Stopped.",
  ]);
});

test("a sync replaces sessions and approvals but keeps what was seen", () => {
  const before: ChatState = play(
    { type: "created", session: session("old") },
    { type: "prompted", sessionId: "old", messageId: "m0", text: "hi" },
  );
  const after = reduceChat(before, {
    type: "sync",
    snapshot: {
      sessions: [session("old"), session("new")],
      permissions: [{ id: "p1", sessionId: "new", action: "shell", resources: [] }],
    },
  });
  expect(after.sessions.map((each) => each.id)).toEqual(["old", "new"]);
  expect(after.permissions).toHaveLength(1);
  expect(after.items.old).toEqual([{ kind: "user", id: "m0", text: "hi" }]);
});

test("events that carry no chat state change nothing", () => {
  const state = play({ type: "created", session: session("a") });
  expect(reduceChat(state, { type: "event", event: { type: "environment.stopped" } })).toBe(state);
});

const asked = (id: string): AgentEvent => ({
  type: "permission.asked",
  id,
  sessionId: s,
  action: "shell",
  resources: [],
});

test("an approval that arrives while the snapshot loads is not lost to it", () => {
  // The snapshot was taken before the approval was asked; its answer reaches the page after the event.
  const state = play(
    { type: "syncing" },
    { type: "event", event: asked("p1") },
    { type: "sync", snapshot: { sessions: [session(s)], permissions: [] } },
  );
  expect(state.permissions.map((permission) => permission.id)).toEqual(["p1"]);
  expect(state.held).toBeNull();
});

test("an answer that arrives while the snapshot loads still removes what the snapshot lists", () => {
  const listed = { id: "p1", sessionId: s, action: "shell", resources: [] };
  const state = play(
    { type: "syncing" },
    {
      type: "event",
      event: { type: "permission.replied", sessionId: s, permissionId: "p1", reply: "once" },
    },
    { type: "sync", snapshot: { sessions: [], permissions: [listed] } },
  );
  expect(state.permissions).toEqual([]);
});

test("held events keep their order, and a failed snapshot still releases them", () => {
  const held = play(
    { type: "created", session: session(s) },
    { type: "syncing" },
    {
      type: "event",
      event: { type: "session.text.delta", sessionId: s, messageId: "m1", delta: "a" },
    },
    { type: "event", event: asked("p1") },
  );
  // Nothing applies while the snapshot loads.
  expect(held.items[s]).toBeUndefined();
  expect(held.permissions).toEqual([]);
  const released = reduceChat(held, { type: "sync" });
  expect(released.items[s]).toEqual([{ kind: "assistant", id: "m1", text: "a" }]);
  expect(released.permissions.map((permission) => permission.id)).toEqual(["p1"]);
  expect(released.sessions).toHaveLength(1);
});
