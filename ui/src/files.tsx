import { useEffect, useState } from "react";
import { api, type FileEntry } from "./client";
import { SafeMarkdown } from "./markdown";
import { Empty, Icon, Notice, useTask } from "./ui";

const basename = (path: string) => path.slice(path.lastIndexOf("/") + 1);
const isMarkdown = (path: string) => /\.md$/i.test(path);

/** A read-only view of /workspace. File contents are untrusted: shown as text or safe Markdown. */
export function Files({ envId }: { envId: string }) {
  const [dir, setDir] = useState("");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  // `text` is null for a file that is not text.
  const [opened, setOpened] = useState<{ path: string; text: string | null } | null>(null);
  const [source, setSource] = useState(false);
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

  const parts = dir === "" ? [] : dir.split("/");
  return (
    <div className="files">
      <section>
        <div className="row">
          <nav className="crumbs" aria-label="Path">
            <button type="button" className="link" onClick={() => list("")}>
              /workspace
            </button>
            {parts.map((name, index) => (
              <span key={parts.slice(0, index + 1).join("/")}>
                <Icon name="chevron" className="faint" />
                <button
                  type="button"
                  className="link"
                  onClick={() => list(parts.slice(0, index + 1).join("/"))}
                >
                  {name}
                </button>
              </span>
            ))}
          </nav>
          <span className="spacer" />
          <button type="button" className="ibtn" aria-label="Refresh" onClick={() => list(dir)}>
            <Icon name="refresh" />
          </button>
        </div>
        {task.error && <Notice tone="bad">{task.error}</Notice>}
        {entries.length === 0 && !task.error ? (
          <Empty icon="folder" title="No files here" />
        ) : (
          <ul className="entries" aria-label="Files">
            {entries.map((entry) => (
              <li key={entry.path}>
                <button
                  type="button"
                  className="entry"
                  aria-current={entry.path === opened?.path}
                  onClick={() => (entry.type === "directory" ? list(entry.path) : open(entry.path))}
                >
                  <Icon name={entry.type === "directory" ? "folder" : "file"} />
                  {basename(entry.path)}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="viewer" aria-label="File">
        {opened === null ? (
          <Empty icon="file" title="Pick a file">
            <p>Files are shown read-only.</p>
          </Empty>
        ) : (
          <>
            <div className="row">
              <Icon name="file" />
              <strong>{opened.path}</strong>
              <span className="spacer" />
              {opened.text !== null && isMarkdown(opened.path) && (
                <label className="check">
                  <input
                    type="checkbox"
                    checked={source}
                    onChange={(event) => setSource(event.target.checked)}
                  />
                  Show source
                </label>
              )}
            </div>
            {opened.text === null ? (
              <Notice>This file is not text and is not shown.</Notice>
            ) : isMarkdown(opened.path) && !source ? (
              <SafeMarkdown body={opened.text} />
            ) : (
              <pre className="file">{opened.text}</pre>
            )}
          </>
        )}
      </section>
    </div>
  );
}
