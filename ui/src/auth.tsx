import qrcode from "qrcode-generator";
import { type ReactNode, useMemo, useState } from "react";
import { api, type DomainStatus, type User } from "./client";
import { DomainForm } from "./domain";
import { Form, Icon, Notice, type Spec } from "./ui";

const Card = ({ title, step, children }: { title: string; step?: string; children: ReactNode }) => (
  <main className="auth">
    <div className="card">
      <h1>
        <Icon name="logo" />
        {title}
      </h1>
      {step && <p className="muted">{step}</p>}
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
        icon="check"
        fields={[username, secret("password", "Password"), otp]}
        onSubmit={async (v) =>
          onDone((await api.login({ username: "", password: "", code: "", ...v })).user)
        }
      />
    </Card>
  );
}

/**
 * First-run wizard: the admin account, then the authenticator, confirmed by a
 * first sign-in, then on the host the domain.
 */
export function Setup({ onDone }: { onDone(user: User): void }) {
  const [enrolled, setEnrolled] = useState<{
    uri: string;
    username: string;
    password: string;
  } | null>(null);
  const [signedIn, setSignedIn] = useState<{ user: User; domain: DomainStatus } | null>(null);
  if (signedIn !== null) {
    return (
      <Card title="Choose the domain" step="Step 3: your own domain, if you have one">
        <DomainForm status={signedIn.domain} action="Use this domain">
          <button type="button" className="btn" onClick={() => onDone(signedIn.user)}>
            Keep https://{signedIn.domain.domain}
          </button>
        </DomainForm>
      </Card>
    );
  }
  if (enrolled === null) {
    return (
      <Card title="Set up Tentaflake" step="Step 1: the admin account">
        <Form
          action="Create admin"
          icon="check"
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
    <Card title="Set up your authenticator" step="Step 2: confirm with a code">
      <Notice tone="warn">
        Scan the code with an authenticator app, or enter the secret. It is shown only now and
        cannot be recovered: without it you cannot sign in.
      </Notice>
      <Qr text={enrolled.uri} />
      <p className="secret-line">
        Secret: <code className="secret">{new URL(enrolled.uri).searchParams.get("secret")}</code>
      </p>
      <Form
        action="Confirm and sign in"
        icon="check"
        fields={[
          { ...otp, label: "Code from the app", hint: "Signing in with it confirms the setup." },
        ]}
        onSubmit={async ({ code = "" }) => {
          const { username, password } = enrolled;
          const { user } = await api.login({ username, password, code });
          const domain = await api.domain();
          if (domain.configurable) setSignedIn({ user, domain });
          else onDone(user);
        }}
      />
    </Card>
  );
}
