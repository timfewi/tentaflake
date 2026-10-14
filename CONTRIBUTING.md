# Contributing to tentaflake

Tentaflake is a generic template. Company configuration, real hostnames,
hardware profiles, API keys and deployment-specific policy belong in forks.

Tentaflake is being reset for 0.5; see [the architecture](docs/architecture.md).
The pre-0.5 line is frozen at tag `v0.4-final` and branch `legacy/0.4`.

## Development setup

```bash
git clone https://github.com/timfewi/tentaflake
cd tentaflake
nix develop
```

The development shell provides Nix tooling, Rust, Cargo, Clippy, Rustfmt,
Statix, Deadnix, actionlint and `just`. The `just` recipes enter this pinned
environment automatically; `just list` lists them.

## Checks

| Recipe | What it does |
|---|---|
| `just fmt` | Format Nix and Rust in place |
| `just fmt-check` | Read-only formatting check |
| `just test` | Unit tests |
| `just fast` | Formatting, lint, Rust lint and tests, offline catalog checks |
| `just check` | `fast` plus `nix flake check --no-build` (the CI gate) |
| `just build` | Build the host closure (explicit, not in CI) |

CI runs `just check` only. Host builds and VM tests are manual steps before a
release, not part of every pull request. Contributions must not activate NixOS,
deploy, or mutate a VM as a side effect of testing.

## Conventions

| Area | Convention |
|---|---|
| Commits | Conventional Commits |
| Nix | `nix fmt` and two-space indent |
| Rust | Rustfmt, Clippy warnings denied |
| Sign-off | DCO on every non-merge commit |
| Size | Pull requests stay small: about 1,500 lines of production code at most |

Create signed-off commits with:

```bash
git commit -s -m "feat: add capability"
```

## Pull requests

1. Link the change to an issue or concrete requirement.
2. Keep the change focused and generic.
3. Add or update focused verification.
4. Keep README, `docs/architecture.md` and the agent instructions in sync.
5. Fill every applicable part of the PR template.

Do not stage, overwrite, or remove unrelated work in a dirty checkout.

## Signing and licensing

Every non-merge commit needs a `Signed-off-by:` line under the Developer
Certificate of Origin in [DCO.txt](DCO.txt). Contributions are provided under
the MIT license. The name and logo are governed separately by
[TRADEMARK.md](TRADEMARK.md).

Report vulnerabilities through GitHub Security Advisories, not public issues.
