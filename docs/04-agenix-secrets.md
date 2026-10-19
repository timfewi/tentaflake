# Agenix secrets and host-held credentials

Agenix stores encrypted files in Git and decrypts them at activation/boot into
runtime files. It does not hide a credential from an agent that receives it.
Balanced/strict reject direct `envFile` and `agenixFile` inputs; secure model
credentials belong only in the host LLM broker.

## Enable the pinned input

In the consumer fork, enable the optional `agenix` input in `flake.nix`, review
and commit its lock revision, and import `inputs.agenix.nixosModules.age` in
the selected host's module list. Use the CLI from that reviewed revision, not
an unrelated moving package.

Two separate Nix files are needed:

| File | Reader | Purpose |
|---|---|---|
| `secrets/secrets.nix` | Agenix CLI, run from `secrets/` | Encryption recipients for each ciphertext |
| Root `secrets.nix` | NixOS module system | Runtime path, owner and permissions |

A NixOS `age.secrets` declaration does not supply the CLI's recipient rules.

## Declare encryption recipients

Create `secrets/secrets.nix` with authenticated public recipients:

```nix
{
  "llm-provider.age".publicKeys = [
    "<host-public-recipient>"
    "<editor-or-recovery-public-recipient>"
  ];
}
```

Replace both placeholders with actual age or supported SSH public keys. The
host must hold a matching private identity; retain a protected recovery identity
outside that host. Confirm a host key through an authenticated management path
and independently verify its fingerprint. `ssh-keyscan` alone does not
authenticate it. Never put private keys into these rules or the repository.

Using the pinned CLI, run from the rules directory:

```sh
cd secrets
agenix -e llm-provider.age
```

Enter the provider key in the editor as a single value, not as an
`OPENAI_API_KEY=...` environment assignment. Do not pass secret values through
command arguments, shell history or printed output. Commit only ciphertext
and non-secret declarations after review.

## Declare the runtime secret

Copy [secrets.nix.example](../secrets.nix.example) to root `secrets.nix` and
import it from `configuration.nix`. Its host-only declaration is:

```nix
{
  age.identityPaths = [ "/var/lib/agenix/identity" ];
  age.secrets.llm-provider = {
    file = ./secrets/llm-provider.age;
    owner = "root";
    group = "root";
    mode = "0400";
  };
}
```

Provision the matching private identity at that runtime path through the
operator's protected credential channel. Use a string, not a Nix path literal:
the private file must not become a store input. Confirm owner-only permissions
and persistence across reboot. Balanced disables OpenSSH, so do not assume an
SSH host key exists automatically.

Encrypted `.age` files can enter the store. Plaintext must never enter Nix
settings, JSON, `extraEnvironment`, seed directories or derivations; do not
use `builtins.readFile` to load a runtime secret.

## Connect only the host broker

In a module with `config` available:

```nix
tentaflake.broker.agents.hermes-coding = {
  enable = true;
  subnet = "10.203.20.0/30";
  gateway = "10.203.20.1";
  llm = {
    enable = true;
    upstreamBaseUrl = "https://api.openai.com/v1/";
    providerCredentialFile = config.age.secrets.llm-provider.path;
    allowedModels = [
      {
        name = "gpt-5-mini";
        inputMicrousdPerMillion = 250000;
        outputMicrousdPerMillion = 2000000;
      }
    ];
  };
};
```

Review the exact model and current prices for your deployment; these are example
policy values. systemd loads the provider value into the broker's private
credential directory. The agent gets a scoped virtual key. This fragment does
not configure the worker, quota or Research required for balanced automatic
startup; see [brokered egress](12-brokered-egress.md) and
[the secure adapter example](../examples/adapter-secure.nix).

Build and review the selected host before an explicitly authorized activation.
A successful source check does not provision identities or decrypt runtime
secrets on the host.

## Verify without revealing values

On the activated host, check the exact path and intended reader:

```sh
sudo stat -Lc '%U %G %a %n' /run/agenix/llm-provider
sudo systemctl status tentaflake-broker-llm-hermes-coding.service --no-pager
tentaflake doctor --security
```

Expect root ownership and mode `400` for this declaration. Check broker
`/healthz` readiness and bounded logs. Local readiness does not prove a provider
call succeeds. Never dump `/run/agenix/*`, decrypted files or OCI environment
data for verification.

## Dev-only direct environment files

A trusted host may explicitly choose `tentaflake.security.profile = "dev"`
and use `agenixFile` on Hermes/ZeroClaw. Declare a separate encrypted environment
file containing the pinned runtime's supported variables; use the same
recipient/module separation and restrict its host reader.

The OCI runtime consumes this file at container creation; it is not a bind
mount and will not appear in `docker inspect .Mounts`. Verify its runtime path,
permissions and the declared environment-file input without exposing values.
Interactive provider setup and direct credentials are not a balanced workflow.

## Rotation and recovery

For a provider-value rotation, edit the ciphertext through the pinned CLI,
review/build, and activate the new secret within the authorized scope. Restart
only the exact LLM broker to refresh systemd's loaded credential. Its virtual key
survives broker restarts. Dev-only environment changes require recreating the
selected controller; use the coordinated Tentaflake lifecycle operation.

When changing recipients, edit the CLI rules and rekey from that directory:

```sh
cd secrets
agenix --rekey
```

Rekeying does not revoke credentials or rewrite Git history. Old ciphertext
remains decryptable with an old recipient key; if that key was compromised,
rotate every exposed secret value separately. Authenticate replacement public
keys before adding them. With no surviving recipient identity, encrypted values
cannot be recovered.

## Troubleshooting

| Symptom | Check |
|---|---|
| CLI finds no recipient rule | Run in `secrets/`; its `secrets.nix` must map the exact ciphertext filename |
| Decryption fails | Matching runtime identity, authenticated recipients and persistent identity path |
| Runtime secret is absent | Agenix module import, selected host and activation result |
| Broker refuses a credential | Exact runtime path, ownership/mode, single-value format and refreshed unit credentials |
| Secret enters the store | Remove Nix value interpolation/path literals; use runtime file paths |

For backup, Git and Grafana, use each host service's documented credential
format and permissions. See [operations](07-operations.md),
[observability](09-observability.md) and the reviewed
[Agenix source](https://github.com/ryantm/agenix).
