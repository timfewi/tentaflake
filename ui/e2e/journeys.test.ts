/**
 * Browser journey through the built UI in headless Chromium: the real control
 * service (auth, CSRF, security headers, session proxy) with a fake Docker and
 * a fake OpenCode behind it, so no container runs and no model is called.
 * Run it with `just ui-e2e`; it is not part of CI.
 */
import { expect, test } from "bun:test";
import { join } from "node:path";
import { type ConsoleMessage, chromium, type Page } from "playwright-core";
import {
  base32Decode,
  harness,
  nextCode,
  password,
  setupToken,
  until,
} from "../../control/test/helpers";

const port = Number(process.env.E2E_PORT ?? 8793);
const base = `http://127.0.0.1:${port}`;
const secret = "sk-ant-journey-0123456789";

/** An OpenCode server-sent event as the fake emits it. */
const opencode = (type: string, fields: object) =>
  `data: ${JSON.stringify({ id: "evt_1", type, location: { directory: "/workspace" }, data: fields, durable: { seq: 1 } })}\n\n`;

async function signIn(page: Page, h: ReturnType<typeof harness>, totp: Buffer) {
  await page.getByLabel("Username").fill("admin");
  await page.getByLabel("Password").fill(password);
  await page.getByLabel("Authenticator code").fill(nextCode(h, totp));
  await page.getByRole("button", { name: "Sign in" }).click();
}

test("setup, login, key, environment, chat with an approval, file, terminal, stop", async () => {
  const h = harness({ ui: join(import.meta.dir, "..", "dist"), port, origin: base });
  const browser = await chromium.launch();
  const page = await browser.newPage();
  // Newer browsers return a Promise from scrolling. It must never become a React cleanup.
  await page.addInitScript(`
    const scroll = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (options) {
      scroll.call(this, options);
      return Promise.resolve();
    };
  `);
  // A CSP violation or any script error shows up here. The sign-in probe of a signed-out page is a 401.
  const problems: string[] = [];
  page.on("pageerror", (error) => problems.push(error.message));
  page.on("console", (message: ConsoleMessage) => {
    if (message.type() === "error" && !message.text().includes("401 (Unauthorized)")) {
      problems.push(message.text());
    }
  });
  try {
    // Setup wizard: account, then the authenticator, confirmed by a first code.
    const response = await page.goto(base);
    const headers = response?.headers() ?? {};
    expect(headers["content-security-policy"]).toContain("frame-ancestors 'none'");
    expect(headers["content-security-policy"]).not.toMatch(/script-src[^;]*unsafe/);
    expect(headers["x-content-type-options"]).toBe("nosniff");
    expect(headers["referrer-policy"]).toBe("no-referrer");
    await page.getByLabel("Setup token").fill(setupToken);
    await page.getByLabel("Admin username").fill("admin");
    await page.getByLabel("Password", { exact: true }).fill(password);
    await page.getByLabel("Repeat password").fill(password);
    await page.getByRole("button", { name: "Create admin" }).click();
    await page.getByRole("heading", { name: "Set up your authenticator" }).waitFor();
    await page.getByRole("img", { name: "Authenticator QR code" }).waitFor();
    const totp = base32Decode((await page.locator(".secret").textContent()) ?? "");
    await page.getByLabel("Code from the app").fill(nextCode(h, totp));
    await page.getByRole("button", { name: "Confirm and sign in" }).click();
    await page.getByRole("heading", { name: "Environments", exact: true }).waitFor();

    // Logout, then login with password and a fresh code.
    await page.getByRole("button", { name: "Sign out" }).click();
    await page.getByRole("heading", { name: "Sign in to Tentaflake" }).waitFor();
    await signIn(page, h, totp);
    await page.getByRole("heading", { name: "Environments", exact: true }).waitFor();

    // A key: the secret goes in once and is not shown again.
    await page.getByRole("link", { name: "Model keys" }).click();
    await page.getByLabel("Label").fill("main");
    await page.getByLabel("Secret").fill(secret);
    await page.getByRole("button", { name: "Add key" }).click();
    await page.getByRole("cell", { name: "main" }).waitFor();
    expect(await page.content()).not.toContain(secret);

    // An environment with an MCP server whose secret the agent can read.
    await page.getByRole("link", { name: "Environments" }).click();
    await page.getByRole("button", { name: "Create environment" }).click();
    await page.getByLabel("Name").fill("demo");
    await page.getByLabel("Models").fill("fixture-model");
    await page.getByRole("button", { name: "Add MCP server" }).click();
    await page.getByLabel("Server name").fill("docs");
    await page.getByLabel("Type").selectOption("remote");
    await page.getByLabel("URL").fill("https://mcp.example/sse");
    await page.getByLabel("Secret headers").fill("Authorization=Bearer token-1");
    await page.getByRole("button", { name: "Create environment", exact: true }).last().click();
    await page.getByText("The environment was created.").waitFor();
    await page.getByText(/^docs: .*can read/).waitFor();
    await page.getByRole("button", { name: "Done" }).click();
    await page.getByRole("link", { name: "demo" }).click();

    // Chat: the prompt, a streamed reply, and an approval the owner answers.
    await page.getByText("Live", { exact: true }).waitFor();
    await page.getByLabel("Message").fill("Write hello.py and run it");
    await page.getByRole("button", { name: "Send" }).click();
    await page.locator(".msg.user").getByText("Write hello.py and run it").waitFor();
    await until(() => h.opencode.requests.some((r) => r.path.endsWith("/prompt")), "the prompt");
    const session = { sessionID: "ses_fake1" };
    const message = { ...session, assistantMessageID: "msg_2" };
    h.opencode.permissions = [
      { id: "per_1", ...session, action: "shell", resources: ["python hello.py"] },
    ];
    h.opencode.emit(
      [
        opencode("session.execution.started", session),
        opencode("session.text.delta", {
          ...message,
          ordinal: 0,
          delta: 'Running <img src=x onerror="window.pwned=1"> ',
        }),
        opencode("session.tool.input.started", { ...message, id: "call_1", name: "shell" }),
        opencode("session.tool.called", {
          ...message,
          id: "call_1",
          input: { command: "python hello.py" },
        }),
        opencode("permission.asked", {
          id: "per_1",
          ...session,
          action: "shell",
          resources: ["python hello.py"],
          save: [],
        }),
      ].join(""),
    );
    await page.getByText("Approval needed: shell").waitFor();
    await page.getByText("python hello.py", { exact: true }).first().waitFor();
    await page.getByRole("button", { name: "Allow once" }).click();
    await until(() => h.opencode.permissions.length === 0, "the approval reply");
    await page.getByText("Approval needed: shell").waitFor({ state: "detached" });
    h.opencode.emit(
      [
        opencode("permission.replied", { ...session, requestID: "per_1", reply: "once" }),
        opencode("session.tool.success", {
          ...message,
          id: "call_1",
          content: [{ type: "text", text: "hello from hello.py\n" }],
          metadata: {},
        }),
        opencode("session.text.ended", {
          ...message,
          ordinal: 0,
          text: 'Running <img src=x onerror="window.pwned=1"> then **hello.py** printed its greeting.',
        }),
        opencode("session.execution.succeeded", session),
      ].join(""),
    );
    await page.getByText("printed its greeting").waitFor();
    await page.getByText("hello from hello.py").waitFor({ state: "attached" });
    // Agent output is text: no element was created from it and no script ran.
    expect(await page.locator(".transcript img").count()).toBe(0);
    expect(await page.evaluate("window.pwned")).toBeUndefined();

    // Files: read-only, Markdown rendered safely.
    h.opencode.files = {
      "hello.py": "print('hello')\n",
      "notes.md":
        "# Notes\n\n<script>window.pwned = 1</script>\n\n![x](https://tracker.example/p.png)\n",
    };
    await page.getByRole("tab", { name: "Files" }).click();
    await page.getByRole("button", { name: "hello.py" }).click();
    await page.getByText("print('hello')").waitFor();
    await page.getByRole("button", { name: "notes.md" }).click();
    await page.getByRole("heading", { name: "Notes" }).waitFor();
    expect(await page.locator(".viewer img, .viewer script").count()).toBe(0);
    expect(await page.evaluate("window.pwned")).toBeUndefined();

    // Terminal: input reaches the shell, output reaches the screen.
    await page.getByRole("tab", { name: "Terminal" }).click();
    await page.locator(".xterm-rows").getByText("$").first().waitFor();
    await page.locator(".xterm").click();
    await page.keyboard.type("ls");
    await until(() => h.opencode.pty.input.join("").includes("ls"), "the terminal input");
    h.opencode.output("hello.py\r\n");
    await page.locator(".xterm-rows").getByText("hello.py").waitFor();

    // Stop: the page follows the environment.
    await page.getByRole("button", { name: "Stop", exact: true }).click();
    await page.getByText("The environment is stopped").waitFor();
    await page.getByRole("button", { name: "Start", exact: true }).waitFor();
    expect([...h.docker.containers.values()].every((container) => !container.running)).toBe(true);

    // Delete asks first.
    await page.getByRole("link", { name: "Environments" }).first().click();
    await page.getByRole("button", { name: "Delete" }).click();
    await page.getByRole("button", { name: "Delete environment" }).click();
    await page.getByText("No environments yet").waitFor();
    expect(h.docker.containers.size).toBe(0);
    expect(problems).toEqual([]);
  } catch (error) {
    await page.screenshot({
      path: join(process.env.E2E_OUT ?? "/tmp", "tentaflake-ui-e2e-failure.png"),
    });
    throw error;
  } finally {
    await browser.close();
  }
}, 120_000);

test("a failed environment can only be deleted", async () => {
  const h = harness({ ui: join(import.meta.dir, "..", "dist"), port, origin: base });
  const { user, totpSecret } = await h.auth.createUser("owner", password, "member");
  const key = h.keys.add(user.id, {
    provider: "anthropic",
    label: "main",
    secret: "sk-ant-x-0123456789",
  });
  const { id } = await h.environments.create(user.id, {
    name: "dev",
    keyId: key.id,
    models: ["fixture-model"],
  });
  h.db.query("UPDATE environments SET state = 'failed' WHERE id = ?").run(id);
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.goto(base);
    await page.getByLabel("Username").fill("owner");
    await page.getByLabel("Password").fill(password);
    await page.getByLabel("Authenticator code").fill(nextCode(h, totpSecret));
    await page.getByRole("button", { name: "Sign in" }).click();
    await page.getByRole("cell", { name: "failed" }).waitFor();
    await page.getByRole("button", { name: "Delete" }).waitFor();
    // The service answers 409 to starting a failed environment, so the list does not offer it.
    expect(await page.getByRole("button", { name: "Start", exact: true }).count()).toBe(0);
    await page.goto(`${base}/#/environments/${id}`);
    await page.getByText("cannot be started").waitFor();
    expect(await page.getByRole("button", { name: "Start", exact: true }).isDisabled()).toBe(true);
  } finally {
    await browser.close();
  }
}, 60_000);
