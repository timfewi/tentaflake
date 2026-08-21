# Fork checklist

Change deployment-specific values in a fork, never in the generic template.

| What | Template location |
|---|---|
| Host and admin defaults | `lib/constants.nix` |
| Host modules | `flake.nix` |
| Agent definitions | `my-agents.nix` |
| Runtime secret paths | Agent builder arguments |
| SSH keys | `tentaflake.adminAuthorizedKeys` |
| Timezone and locale | `tentaflake.*` options |

The Rust package name and workspace live in `Cargo.toml` and
`crates/tentaflake-cli/Cargo.toml`. If a fork renames the project, update those
manifests, the Nix package wrapper, CLI paths, documentation, and tests together.

## Pinning

The repository tracks `nixos-unstable` at the exact revision in `flake.lock`.
Update deliberately, review the lock diff, and rerun the flake and VM checks.
Do not weaken package security policy merely to retain an old pin.

## Secrets

Create encrypted secret declarations in the fork. Keep the decryption identity
off-host in a protected recovery location. Literal secret values must not enter
Nix expressions, JSON, Git, logs, or build outputs.

## Backend choice

Docker is the default. Under `balanced`, the operator is deliberately not in
its root-equivalent group; narrow CLI actions use sudo. `dev` retains group
access for compatibility. Podman does not add that group, but root-managed
systemd containers and the operator's rootless store are separate. Verify CLI
behavior against the selected backend.

## Security profile

Keep the installed default `balanced` for an untrusted 24/7 workload. It is
fail-closed and has no external connectivity until an exact per-agent broker
policy is declared.
Select `dev` only when a trusted development workflow explicitly needs direct
env credentials, host networking, ports, or mutable local images. `strict`
currently fails evaluation and must not be advertised as deployed.

Enable and migrate to `tentaflake.workspaceQuota.agents.<container>` for every
persistent controller workspace that needs a hard ceiling. The generic module
uses an exact-size ext4 backing image and refuses to hide existing non-empty
data; runtime-specific state outside the workspace still needs capacity
monitoring.

## Optional profiles

Import observability, Falco, Hive Research, Piper TTS, or editor support only
when the deployment needs them. Each integration has a separate package,
credential, network, or privilege boundary; see
[observability and detection](09-observability.md).
