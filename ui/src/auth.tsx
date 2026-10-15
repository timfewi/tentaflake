import qrcode from "qrcode-generator";
import { type ReactNode, useMemo, useState } from "react";
import { api, type User } from "./client";
import { Form, type Spec } from "./ui";

const Card = ({ title, children }: { title: string; children: ReactNode }) => (
  <main className="auth">
    <div className="card">
      <h1>{title}</h1>
      {children}
    </div>
  </main>
);

function Qr({ text }: { text: string }) {
  const { size, path } = useMemo(() => {
    const code = qrcode(0, "M");
    code.addData(text);
    code.make();
    const size = code.getModuleCount();
    let path = "";
    for (let row = 0; row < size; row++) {
      for (let column = 0; column < size; column++) {
        if (code.isDark(row, column)) path += `M${column} ${row}h1v1h-1z`;
      }
    }
    return { size, path };
  }, [text]);
  return (
    <svg
      className="qr"
      viewBox={`-2 -2 ${size + 4} ${size + 4}`}
      role="img"
      aria-label="Authenticator QR code"
    >
      <rect x="-2" y="-2" width={size + 4} height={size + 4} fill="#fff" />
      <path d={path} />
    </svg>
  );
}

const username: Spec = { name: "username", label: "Username", input: { autoComplete: "username" } };
const secret = (name: string, label: string, hint?: string): Spec => ({
  name,
  label,
  hint,
  input: { type: "password", autoComplete: name === "password" ? "current-password" : "off" },
});
const otp: Spec = {
  name: "code",
  label: "Authenticator code",
  input: { inputMode: "numeric", autoComplete: "one-time-code", maxLength: 6 },
};

/** Password + TOTP login. */
export function Login({ onDone }: { onDone(user: User): void }) {
  return (
    <Card title="Sign in to Tentaflake">
      <Form
        action="Sign in"
        fields={[username, secret("password", "Password"), otp]}
        onSubmit={async (v) =>
          onDone((await api.login({ username: "", password: "", code: "", ...v })).user)
        }
      />
    </Card>
  );
}

/** First-run wizard: the admin account, then the authenticator, confirmed by a first sign-in. */
export function Setup({ onDone }: { onDone(user: User): void }) {
  const [enrolled, setEnrolled] = useState<{
    uri: string;
    username: string;
    password: string;
  } | null>(null);
  if (enrolled === null) {
    return (
      <Card title="Set up Tentaflake">
        <Form
          action="Create admin"
          fields={[
            secret("token", "Setup token", "From the host's setup token file."),
            {
              ...username,
              label: "Admin username",
              hint: "Lowercase letters, digits, dots, dashes and underscores.",
              input: { autoComplete: "username", pattern: "[a-z0-9][a-z0-9._\\-]{0,31}" },
            },
            {
              ...secret("password", "Password", "At least 12 characters."),
              input: { type: "password", autoComplete: "new-password", minLength: 12 },
            },
            secret("repeat", "Repeat password"),
          ]}
          problem={(v) =>
            v.repeat && v.repeat !== v.password ? "The passwords differ." : undefined
          }
          onSubmit={async ({ token = "", username = "", password = "" }) => {
            const { otpauthUri } = await api.setup({ token, username, password });
            setEnrolled({ uri: otpauthUri, username, password });
          }}
        />
      </Card>
    );
  }
  return (
    <Card title="Set up your authenticator">
      <p className="notice warn">
        Scan the code with an authenticator app, or enter the secret. It is shown only now and
        cannot be recovered: without it you cannot sign in.
      </p>
      <Qr text={enrolled.uri} />
      <p>
        Secret: <code className="secret">{new URL(enrolled.uri).searchParams.get("secret")}</code>
      </p>
      <Form
        action="Confirm and sign in"
        fields={[
          { ...otp, label: "Code from the app", hint: "Signing in with it confirms the setup." },
        ]}
        onSubmit={async ({ code = "" }) => {
          const { username, password } = enrolled;
          onDone((await api.login({ username, password, code })).user);
        }}
      />
    </Card>
  );
}
