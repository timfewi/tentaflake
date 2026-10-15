import { useEffect, useState } from "react";
import { api, type ModelKey, type Provider } from "./client";
import { Confirm, Empty, Form, Icon, Notice, useTask } from "./ui";

const providers: Provider[] = ["anthropic", "openai", "openrouter", "openai-compatible"];

/** Model API keys: the secret goes in once and is never shown again. */
export function Keys() {
  const [keys, setKeys] = useState<ModelKey[] | null>(null);
  const [removing, setRemoving] = useState<ModelKey | null>(null);
  const loading = useTask();
  const load = () => loading.run(async () => setKeys(await api.keys()));
  // biome-ignore lint/correctness/useExhaustiveDependencies: load once
  useEffect(() => void load(), []);

  return (
    <div className="page">
      <h1>Model keys</h1>
      {loading.error && <Notice tone="bad">{loading.error}</Notice>}
      {keys?.length === 0 && (
        <Empty icon="key" title="No keys yet">
          <p>Add a provider key to create environments.</p>
        </Empty>
      )}
      {keys && keys.length > 0 && (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Label</th>
                <th>Provider</th>
                <th>Base URL</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {keys.map((key) => (
                <tr key={key.id}>
                  <td>
                    <strong>{key.label}</strong>
                  </td>
                  <td>
                    <span className="chip">{key.provider}</span>
                  </td>
                  <td>{key.baseUrl ?? "–"}</td>
                  <td className="actions">
                    <button type="button" className="btn danger" onClick={() => setRemoving(key)}>
                      <Icon name="trash" />
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="card">
        <h2>Add a key</h2>
        <Form
          action="Add key"
          icon="plus"
          initial={{ provider: "anthropic" }}
          fields={[
            { name: "provider", label: "Provider", options: providers.map((name) => [name, name]) },
            { name: "label", label: "Label", input: { maxLength: 64 } },
            {
              name: "baseUrl",
              label: "Base URL",
              hint: "An HTTPS API, e.g. https://llm.example/v1",
              input: { type: "url" },
              show: (v) => v.provider === "openai-compatible",
            },
            {
              name: "secret",
              label: "Secret",
              hint: "Stored encrypted on the host and never shown again. Environments never receive it.",
              input: { type: "password", autoComplete: "off" },
            },
          ]}
          onSubmit={async ({ provider = "", label = "", secret = "", baseUrl = "" }) => {
            const body = { provider: provider as Provider, label, secret };
            await api.addKey(provider === "openai-compatible" ? { ...body, baseUrl } : body);
            setKeys(await api.keys());
          }}
        />
      </div>
      {removing && (
        <Confirm
          title={`Delete ${removing.label}?`}
          action="Delete key"
          onClose={() => setRemoving(null)}
          onConfirm={async () => {
            await api.deleteKey(removing.id);
            setKeys(await api.keys());
          }}
        >
          <p>
            Environments that use this key lose their model access. The secret cannot be recovered.
          </p>
        </Confirm>
      )}
    </div>
  );
}
