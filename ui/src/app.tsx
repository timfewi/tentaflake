import { useEffect, useState, useSyncExternalStore } from "react";
import { Login, Setup } from "./auth";
import { ApiFailure, api, errorMessage, type User, whenSignedOut } from "./client";
import { EnvironmentPage } from "./environment";
import { Environments } from "./environments";
import { Keys } from "./keys";
import { Notice } from "./ui";

type Boot = "loading" | "setup" | "login" | { user: User } | { error: string };

const subscribe = (notify: () => void) => {
  addEventListener("hashchange", notify);
  return () => removeEventListener("hashchange", notify);
};

export function App() {
  const [boot, setBoot] = useState<Boot>("loading");
  const hash = useSyncExternalStore(subscribe, () => location.hash);
  const start = async () => {
    setBoot("loading");
    try {
      if ((await api.setupStatus()).needed) return setBoot("setup");
      setBoot({ user: (await api.session()).user });
    } catch (error) {
      const signedOut = error instanceof ApiFailure && error.code === "unauthenticated";
      setBoot(signedOut ? "login" : { error: errorMessage(error) });
    }
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: boot once
  useEffect(() => {
    whenSignedOut(() => setBoot("login"));
    void start();
  }, []);

  if (boot === "loading") return <p className="muted pad">Loading…</p>;
  if (boot === "setup") return <Setup onDone={(user) => setBoot({ user })} />;
  if (boot === "login") return <Login onDone={(user) => setBoot({ user })} />;
  if ("error" in boot) {
    return (
      <div className="pad">
        <Notice tone="bad">{boot.error}</Notice>
        <button type="button" className="btn" onClick={() => void start()}>
          Try again
        </button>
      </div>
    );
  }

  const [, section = "environments", id] = hash.split("/");
  const link = (name: string, text: string) => (
    <a href={`#/${name}`} aria-current={section === name ? "page" : undefined}>
      {text}
    </a>
  );
  return (
    <>
      <header className="bar">
        <strong>Tentaflake</strong>
        <nav>
          {link("environments", "Environments")}
          {link("keys", "Model keys")}
        </nav>
        <span className="spacer" />
        <span className="muted">{boot.user.username}</span>
        <button
          type="button"
          className="btn"
          onClick={async () => {
            await api.logout().catch(() => {});
            // A reload leaves no file text or terminal output of this session in the page.
            location.hash = "";
            location.reload();
          }}
        >
          Sign out
        </button>
      </header>
      <main>
        {section === "keys" ? (
          <Keys />
        ) : id ? (
          <EnvironmentPage key={id} id={id} />
        ) : (
          <Environments />
        )}
      </main>
    </>
  );
}
