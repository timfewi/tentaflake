import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";
import { terminalUrl } from "./client";
import { Notice } from "./ui";

// The service drops a connection that sends more than 64 KiB in one frame.
const chunk = 16 * 1024;

/**
 * A shell in the environment. The emulator draws the output into its own grid:
 * bytes from the environment are never inserted as markup.
 */
export function TerminalView({ envId, active }: { envId: string; active: boolean }) {
  const host = useRef<HTMLDivElement>(null);
  const fit = useRef<() => void>(() => {});
  const [closed, setClosed] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new shell per environment and per reconnect
  useEffect(() => {
    const element = host.current;
    if (!element) return;
    setClosed(null);
    const terminal = new Terminal({
      cursorBlink: true,
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
    socket.onopen = () => fit.current();
    socket.onmessage = (message) =>
      typeof message.data === "string" && terminal.write(message.data);
    socket.onclose = (event) => setClosed(event.reason || "the connection closed");
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

  return (
    <div>
      {closed !== null && (
        <Notice tone="warn">
          Terminal closed: {closed}.{" "}
          <button type="button" className="link" onClick={() => setAttempt(attempt + 1)}>
            Open a new shell
          </button>
        </Notice>
      )}
      <div className="screen" ref={host} />
    </div>
  );
}
