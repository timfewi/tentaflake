# Encrypted secrets

This directory holds encrypted `.age` files for
[Agenix](https://github.com/ryantm/agenix). Plaintext environment files, private
keys and unencrypted credentials must never be committed. Deployment secrets
and recipient identities belong in the deployment fork.

## Balanced deployments

Encrypt a single provider value in a reviewed recipient setup, declare the
host-side secret, and point the LLM broker at its runtime path:

```nix
age.secrets.provider-key = {
  file = ./secrets/provider-key.age;
  owner = "root";
  group = "root";
  mode = "0400";
};

# Within an enabled, otherwise complete per-agent broker declaration:
tentaflake.broker.agents.hermes-assistant.llm.providerCredentialFile =
  config.age.secrets.provider-key.path;
```

The broker expects the provider value, not a `KEY=value` environment file. The
agent receives only a virtual broker key. See the
[Agenix guide](../docs/04-agenix-secrets.md) and
[broker guide](../docs/12-brokered-egress.md) for the complete setup.

Verify paths and permissions without displaying contents:

```bash
stat -c '%U %G %a %n' /run/agenix/provider-key
```

## Dev compatibility

Encrypted `*.env.age` files can supply `agenixFile` directly to a runtime only
when the host deliberately selects the `dev` profile. Balanced rejects both
`envFile` and `agenixFile` on the agent. Keep direct-provider examples separate
from secure deployment instructions; dev is not an untrusted 24/7 boundary.

Use `agenix -e <encrypted-file>` from your reviewed, pinned agenix setup to
create or edit a secret. Keep at least one protected recovery recipient and
rotate secret values as well as recipients when a key is compromised. See
[secrets.nix.example](../secrets.nix.example) for the recipient template.
