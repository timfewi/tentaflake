# Agent Instructions — tentaflake

Tentaflake 0.5: a NixOS host where a web UI creates isolated agent
environments (Ubuntu in gVisor, one agent runtime, the users' own API keys,
MCP servers and tools). Read [docs/architecture.md](docs/architecture.md)
first; it holds the model, the boundaries, the definition of done and the
status. The pre-0.5 line is frozen at `v0.4-final` / `legacy/0.4`; do not
port code from it unless the current task needs that exact piece now.

## Build & Test

`just` recipes load the pinned contributor tools automatically.

```bash
just fmt         # format Nix and Rust in place
just fmt-check   # read-only formatting check
just test        # unit tests
just fast        # formatting, lint, Rust lint and tests, offline catalog checks
just check       # fast + nix flake check --no-build (the CI gate)
just build       # build the host closure (explicit, not in CI)
```

CI runs `just check` only. Host builds and VM tests are manual steps before a
release; never add them to CI or run them as a routine check.

## Working rules

- One agent works on the repository at a time; no helper agents.
- Small pull requests: about 1,500 lines of production code at most
  (deletions do not count). Stop after each PR for the owner's review.
- Build only what the definition of done in `docs/architecture.md` needs.
  No speculative abstractions, no parallel tracks.
- Keep docs small: README, `docs/architecture.md`, CHANGELOG. Update the
  architecture status table when a part lands; update the CHANGELOG once per
  release, not per commit.

## Conventions

- Nix: `nix fmt` (nixfmt), 2-space indent; `statix` and `deadnix` clean.
- Rust: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`.
- Commits: Conventional Commits (`feat:`, `fix:`, `docs:`).
- DCO: every non-merge PR commit needs `Signed-off-by:`; use `git commit -s`
  with the human user's configured Git author and committer identity. Never
  replace that identity with an agent identity.

## Public repository rule

This repository is public and generic. Never put private data into it in any
form: no names, email addresses, host or machine names, tailnet names, IP
addresses, local paths, credentials, tokens or session links — in code,
commits, branch names, PRs, issues, comments, docs, fixtures or logs.
Deployment-specific configuration belongs in forks.

## Module boundaries

- `modules/` — host NixOS modules, imported by `modules/default.nix`.
  `security.nix` owns the security profile, environment resource defaults and
  the gVisor `runsc` runtime registration.
- `configuration.nix` — host defaults: Docker as the container backend; the
  Docker socket is never granted to the login user in secure profiles.
- `crates/tentaflake-broker`, `pkgs/tentaflake-broker` — the model broker;
  `lib/mkRustPackage.nix` packages it.
- `images/catalog.json` — reviewed base-image pins with offline-checked
  evidence (`just image-catalog`).
- `ui/` and the control service — planned; see the architecture status.

## PR Description

Fill out `.github/PULL_REQUEST_TEMPLATE.md`: a real `## Description` (what
changed and why), the applicable `Type of Change` box, and every applicable
checklist item.
