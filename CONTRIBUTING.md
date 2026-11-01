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

Run the relevant local gates for the change: `just eval` for host
configuration, `just generated-flake` for installer output, and affected package,
ISO or VM builds when authorized. `just host-api-test` covers Unix API protocol
fixtures. See [build boundaries](docs/17-builds.md) for exact commands, sequential
projection/Research closure preparation, CI routing and end-to-end recipes.
`just ci`/`just e2e` are larger workloads, not aliases for fast checks.

Every PR/push CI run executes all affected gates, including package/system
builds and VM tests; unknown paths/history select the full inventory. Every
selected job must pass for this revision before merge. Synthetic probes are
not vendor acceptance.
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
`docs/` contains only product documentation listed in the
[guide inventory](docs/18-documentation.md#guide-inventory) (checked by
`just fast` and CI). Plans, backlogs, prompts, review or session notes and
private deployment facts never enter Git; keep them in the ignored `.scratch/`
directory. Identify guide additions/removals for maintainers, without requiring
private website access or claiming unreleased changes are published.

Core modules belong in `modules/` and its default import; optional profiles need
explicit flake exports and documented trust boundaries. Consumer-specific
editor, Hive Research and speech integrations stay in forks. Adding a runtime
must reuse shared containment, not establish a parallel security implementation.
Public instance examples use `tentaflake.toml`; consumer Nix modules may only load
that file through file-only `systemFromData`. Keep internal builders out of the
public API and host policy separate from instance definitions. Keep the shipped
inventory empty and commented (`schemaVersion = 1`, `agents = []`); active
installation data belongs in a consumer/fork-owned file, not the upstream input.
Track that file in the owning Git source; missing/untracked desired state must
fail, never fall back to the upstream template. See
[installation-owned configuration](docs/08-agent-cli.md#installation-owned-configuration).
No separate example inventory is shipped. Legacy TOML/JSON requires deliberate migration.

Contributions use the [MIT license](LICENSE) and [DCO](DCO.txt); name/logo terms
are in [TRADEMARK.md](TRADEMARK.md). Report vulnerabilities privately through
GitHub Security Advisories, not public issues.
