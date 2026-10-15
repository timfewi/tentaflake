import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";
import { terminalUrl } from "./client";
import { cx, Icon, Notice } from "./ui";

// The service drops a connection that sends more than 64 KiB in one frame.
const chunk = 16 * 1024;

type Link = "connecting" | "open" | { closed: string };

/**
 * A shell in the environment. The emulator draws the output into its own grid:
 * bytes from the environment are never inserted as markup.
 */
export function TerminalView({
  envId,
  name,
  active,
}: {
  envId: string;
  name: string;
  active: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);
  const fit = useRef<() => void>(() => {});
  const [link, setLink] = useState<Link>("connecting");
  const [attempt, setAttempt] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new shell per environment and per reconnect
  useEffect(() => {
    const element = host.current;
    if (!element) return;
    setLink("connecting");
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: 'ui-monospace, "JetBrains Mono", Menlo, Consolas, monospace',
      fontSize: 13,
      scrollback: 5000,
      theme: { background: "#0d0f14", foreground: "#e4e7ee", cursor: "#8b7cf8" },
    });
    const addon = new FitAddon();
    terminal.loadAddon(addon);
    terminal.open(element);
    const socket = new WebSocket(terminalUrl(envId));
    const frame = (message: object) =>
      socket.readyState === WebSocket.OPEN && socket.send(JSON.stringify(message));
    fit.current = () => {
      if (element.clientWidth === 0) return;
      addon.fit();
      frame({ type: "resize", rows: terminal.rows, cols: terminal.cols });
    };
    socket.onopen = () => {
      setLink("open");
      fit.current();
    };
    socket.onmessage = (message) =>
      typeof message.data === "string" && terminal.write(message.data);
    socket.onclose = (event) => setLink({ closed: event.reason || "the connection closed" });
    const input = terminal.onData((data) => {
      for (let start = 0; start < data.length; start += chunk) {
        frame({ type: "input", data: data.slice(start, start + chunk) });
      }
    });
    const observer = new ResizeObserver(() => fit.current());
    observer.observe(element);
    return () => {
      observer.disconnect();
      input.dispose();
      socket.close();
      terminal.dispose();
    };
  }, [envId, attempt]);

  useEffect(() => {
    if (active) fit.current();
  }, [active]);

  const closed = typeof link === "object";
  return (
    <div className="terminal-view">
      <div className="row">
        <Icon name="terminal" />
        <strong>{name}</strong>
        <span className="muted">/workspace</span>
        <span className="spacer" />
        <span className={cx("chip", link === "open" ? "done" : closed ? "failed" : "running")}>
          {link === "open" ? "Connected" : closed ? "Closed" : "Connecting…"}
        </span>
      </div>
      {closed && (
        <Notice tone="warn">
          Terminal closed: {link.closed}.{" "}
          <button type="button" className="link" onClick={() => setAttempt(attempt + 1)}>
            Open a new shell
          </button>
        </Notice>
      )}
      <div className={cx("screen", link === "open" && "live")} ref={host} />
    </div>
  );
}
