import { type ReactNode, useEffect, useState } from "react";
import { api, type DomainStatus } from "./client";
import { Form, Notice, useTask } from "./ui";

/**
 * Chooses the domain the host serves. The service follows at once, so after a
 * change this page's address stops working: it links to the new one.
 */
export function DomainForm({
  status,
  action,
  children,
}: {
  status: DomainStatus;
  action: string;
  /** Shown below the form until the domain changed. */
  children?: ReactNode;
}) {
  const [moved, setMoved] = useState<string | null>(null);
  if (moved !== null) {
    return (
      <Notice tone="ok">
        Tentaflake now answers on <a href={`https://${moved}/`}>https://{moved}</a>. Open it and
        sign in again; its certificate can take a minute.
      </Notice>
    );
  }
  return (
    <>
      <p>
        Tentaflake answers on <strong>https://{status.domain}</strong>
        {status.custom ? "." : ", a name derived from the server's address."}
      </p>
      <Form
        action={action}
        icon="check"
        initial={{ domain: status.custom ? status.domain : "" }}
        fields={[
          {
            name: "domain",
            label: "Your domain",
            hint: status.address
              ? `Set its A record to ${status.address} first. Empty returns to the derived name.`
              : "Empty returns to the derived name.",
            input: { required: false, placeholder: "agents.example.com", autoComplete: "off" },
          },
        ]}
        onSubmit={async ({ domain = "" }) => {
          const next = await api.setDomain({ domain: domain || null });
          if (next.domain !== status.domain) setMoved(next.domain);
        }}
      />
      {children}
    </>
  );
}

/** The admin's page for the domain. */
export function Domain() {
  const [status, setStatus] = useState<DomainStatus | null>(null);
  const loading = useTask();
  // biome-ignore lint/correctness/useExhaustiveDependencies: load once
  useEffect(() => void loading.run(async () => setStatus(await api.domain())), []);
  return (
    <div className="page">
      <h1>Domain</h1>
      {loading.error && <Notice tone="bad">{loading.error}</Notice>}
      {status && (
        <div className="card">
          {status.configurable ? (
            <DomainForm status={status} action="Use this domain" />
          ) : (
            <p>The service configuration fixes its address: {status.domain}.</p>
          )}
        </div>
      )}
    </div>
  );
}
