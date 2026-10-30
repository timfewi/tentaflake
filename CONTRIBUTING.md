# Contributing

Keep Tentaflake generic. Deployment policy, identities, hardware, private agent
context and secrets belong in forks. Read [AGENTS.md](AGENTS.md) for architecture
and security contracts before changing code.

## Development and checks

Clone the repository and enter `nix develop` for direct Cargo or lint commands.
`just` recipes load the pinned tools automatically; `just list` lists them.
The locked Dev Container is an alternative and mounts neither runtime sockets
nor credentials automatically.

Start with `just fast`: formatting/lint, ShellCheck, Rust unit checks, optional
host API tests, CI routing/workflow checks and pure-preset `catalog-check`.
Edit runtime facts in `adapters/<id>.nix`, then run
`python3 scripts/runtime-catalog-docs.py` to regenerate CLI JSON and the docs
catalog. Generated projections are not independently editable inputs.

Run the relevant explicit gates for the change: `just eval` for host
configuration, `just generated-flake` for installer output, and affected package,
ISO or VM builds when authorized. `just host-api-test` covers Unix API protocol
fixtures. See [build boundaries](docs/17-builds.md) for exact commands, sequential
projection/Research closure preparation, CI dispatch routing and end-to-end
recipes. `just ci`/`just e2e` are larger workloads, not aliases for fast checks.

PR/push CI selects affected fast checks; explicit dispatch selects affected
build/runtime gates too. Unknown paths/history select the full relevant set.
Before merge, obtain required build/runtime evidence for this revision: green
static checks, skipped builds and synthetic probes are not vendor acceptance.
Evaluation, build, activation and live acceptance are distinct; tests never
implicitly deploy or authorize activation.

`just security` separately runs current OSV dependency advisory queries and the
pinned offline Semgrep rule snapshot. It needs network for OSV. Report exercised
checks and failures/unknown evidence rather than claiming unrun gates passed.

## Conventions and review

- Nix: two spaces and nixfmt. `just fmt-check` is read-only; `just fmt` rewrites.
  Treefmt's `--ci` may rewrite before failing.
- Rust: Rustfmt, Clippy warnings denied, workspace tests. Shared packaging uses
  root workspace manifests plus selected crate source; add required root build
  inputs explicitly without coupling unrelated crate caches.
- Shell: ShellCheck. Installer disk changes preserve selected-device boundaries.
- Commits: Conventional Commits and DCO sign-off on each non-merge commit, using
  the human contributor's configured author/committer identity.

For a PR, name the requirement, keep scope focused, add appropriate regressions,
review the diff and synchronize docs/examples/instructions/shared skills. Fill
all applicable sections of `.github/PULL_REQUEST_TEMPLATE.md`, leaving
inapplicable checklist entries unchecked. Preserve unrelated dirty work.

## Documentation, modules and disclosure

The public repository owns guides; the website mirrors a pinned release.
Identify guide additions/removals for maintainers, without requiring private
website access or claiming unreleased changes are published. See
[documentation ownership](docs/18-documentation.md).

Core modules belong in `modules/` and its default import; optional profiles need
explicit flake exports and documented trust boundaries. Consumer-specific
editor, Hive Research and speech integrations stay in forks. Adding a runtime
must reuse shared containment, not establish a parallel security implementation.
Public instance examples use `agents.json`; consumer Nix modules may only load
that file through file-only `agentsFromData`. Keep internal builders out of the
public API and host policy separate from instance definitions.

Contributions use the [MIT license](LICENSE) and [DCO](DCO.txt); name/logo terms
are in [TRADEMARK.md](TRADEMARK.md). Report vulnerabilities privately through
GitHub Security Advisories, not public issues.
