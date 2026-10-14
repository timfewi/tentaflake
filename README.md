# Tentaflake

![Tentaflake octopus and snowflake logo with agent mascots](public/tentaflake-readme.jpg)

> [!WARNING]
> **0.5 reset in progress.** Tentaflake is pre-1.0 and is being rebuilt from a
> small core. Nothing on `main` is ready to deploy yet. The previous line is
> frozen at tag `v0.4-final` and branch `legacy/0.4` and is no longer
> maintained.

Tentaflake turns a rented VPS into a private agent cloud: boot the Tentaflake
image, open the web UI, and create isolated agent environments — an Ubuntu
container in a gVisor sandbox with an agent inside, your own model API keys,
MCP servers and tools. Like an agent in the cloud, but on your own server.

You bring the VPS and the keys. Tentaflake brings the environment and the
simplicity.

## How it works

- **One host, built with NixOS.** Hardened, reproducible, updated with rollback.
- **Environments as data.** The UI creates, starts, stops and deletes
  environments; no host rebuild per environment.
- **One runtime in 0.5:** [OpenCode](https://github.com/anomalyco/opencode),
  driven over its HTTP API, with any model provider.
- **Your keys stay on the host.** Model calls go through the broker; no
  provider key enters an environment.
- **Built for teams later.** 0.5 has one admin, with a multi-user data model
  from the start.

Details and the current status: [architecture](docs/architecture.md).

## Development

```bash
nix develop
just check   # fast checks plus flake evaluation, the CI gate
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the recipes and conventions.

## License

MIT, see [LICENSE](LICENSE). The name and logo follow
[TRADEMARK.md](TRADEMARK.md). Report vulnerabilities as described in
[SECURITY.md](SECURITY.md).
