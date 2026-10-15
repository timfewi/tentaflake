import { useEffect, useState } from "react";
import { api, type FileEntry } from "./client";
import { SafeMarkdown } from "./markdown";
import { Notice, useTask } from "./ui";

/** A read-only view of /workspace. File contents are untrusted: shown as text or safe Markdown. */
export function Files({ envId }: { envId: string }) {
  const [dir, setDir] = useState("");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  // `text` is null for a file that is not text.
  const [opened, setOpened] = useState<{ path: string; text: string | null } | null>(null);
  const task = useTask();

  const list = async (path: string) => {
    const ok = await task.run(async () => {
      const found = await api.files(envId, path);
      const rank = (entry: FileEntry) => (entry.type === "directory" ? 0 : 1);
      setEntries(found.sort((a, b) => rank(a) - rank(b) || a.path.localeCompare(b.path)));
    });
    if (ok) setDir(path);
  };
  const open = (path: string) =>
    task.run(async () => {
      const bytes = await api.fileContent(envId, path);
      try {
        if (bytes.includes(0)) throw new Error("binary");
        setOpened({ path, text: new TextDecoder("utf-8", { fatal: true }).decode(bytes) });
      } catch {
        setOpened({ path, text: null });
      }
    });
  // biome-ignore lint/correctness/useExhaustiveDependencies: list the root once per environment
  useEffect(() => void list(""), [envId]);

  return (
    <div className="files">
      <section>
        <div className="row">
          <strong>/workspace{dir && `/${dir}`}</strong>
          <span className="spacer" />
          <button
            type="button"
            className="btn"
            disabled={dir === ""}
            onClick={() => list(dir.slice(0, Math.max(dir.lastIndexOf("/"), 0)))}
          >
            Up
          </button>
          <button type="button" className="btn" onClick={() => list(dir)}>
            Refresh
          </button>
        </div>
        {task.error && <Notice tone="bad">{task.error}</Notice>}
        <ul aria-label="Files">
          {entries.map((entry) => (
            <li key={entry.path}>
              <button
                type="button"
                onClick={() => (entry.type === "directory" ? list(entry.path) : open(entry.path))}
              >
                {entry.type === "directory" ? "▸ " : ""}
                {entry.path.slice(entry.path.lastIndexOf("/") + 1)}
              </button>
            </li>
          ))}
        </ul>
        {entries.length === 0 && <p className="muted">No files here.</p>}
      </section>
      <section aria-label="File">
        {opened && <strong>{opened.path}</strong>}
        {opened?.text === null && <Notice>This file is not text and is not shown.</Notice>}
        {typeof opened?.text === "string" &&
          (/\.md$/i.test(opened.path) ? (
            <SafeMarkdown body={opened.text} />
          ) : (
            <pre>{opened.text}</pre>
          ))}
      </section>
    </div>
  );
}
