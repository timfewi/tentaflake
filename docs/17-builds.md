# Build boundaries and contributor checks

`just` loads pinned contributor tools; direct Cargo/lint commands need
`nix develop`. Tool realization may download/build dependencies even for a
source check. Evaluation, builds, activation and live acceptance are separate.
No check or successful build authorizes host activation.

## Choose the gate

| Gate | Evidence |
|---|---|
| `just fast` | Formatting/lint, ShellCheck, Rust format/Clippy/unit checks, optional Unix host API tests, routing/workflow checks, small no-build fixtures and catalog drift |
| `just catalog-check` | Pure adapter presets agree with generated CLI JSON/docs, using read-only `nix-instantiate --eval --strict --json --readonly-mode`; no nixpkgs/host build |
| `just fmt-check` | Pinned `nixfmt --check` over Git-selected regular Nix files, no writes |
| `just eval` | Sequential complete flake projections after exact non-VM Research closure and Rust source-fileset preparation |
| `just generated-flake` | Installer-generated consumer evaluation |
| `just host-api-test` | Unix API protocol with temporary state/fake host commands |
| `just security` | Current OSV dependency advisories plus pinned offline Semgrep rules |
| `just ci` / `just e2e` | Complete local gate including VM suites and installer ISO |
| `just e2e-devcontainer` | Locked Dev Container rebuild and lint smoke test |
| `just e2e-installer` / `just e2e-run-vm` | Install/boot an isolated QCOW2 VM, never a host block device |

Fast checks do not evaluate host configurations/generated flakes, prepare
Research closures or build runtime images/packages/systems/VMs. `just fmt`
rewrites; treefmt's `--ci` can write before failing. `just formatter-test` uses
real tracked/untracked fixtures to verify read-only refusal preserves bytes.
Dev Container updates also need their package build to catch offline-cache drift.

## Configuration and runtime fixtures

CI and `just eval` use `scripts/eval_flake.py`. It discovers the complete output
inventory, then runs sequential `nix flake check --no-build` projections: at
most four checks, one host with matching check/package aliases, or a remaining
output group. Each evaluator exits before the next. Keep all outputs/checks,
existing pins and ordinary Git source filtering; only the temporary wrapper
writes a lock. `shallow=1` supports CI's depth-one source. The real-Git fixture
checks clean/dirty tracked sources, ignored/untracked exclusion, preserved
input/lock bytes and upstream rejection of malformed outputs without builds.
This bounds retained evaluation, not each fixture's memory or duration.

Research evaluation needs exact closure derivations prepared first. The Rust
source-isolation check also reads generated filesets, which must exist in a cold
store before read-only evaluation. `just eval` and CI prepare these non-VM
checks without compiling Rust packages. Do not expose the whole Nix store or
remove source checks to make evaluation pass. The normal full flake check
permits required builds.

Focused adapter checks cover common IDs, TOML rejection/validation, lazy
selection, metadata, scaffold refusal and quota-helper negative cases.
`checks.*.hardening-journald` covers defaults/overrides and legacy configuration
refusal. `checks.*.worker-drain-wakeup` uses a synthetic host and real API sandbox
to cover queue-unlock/oneshot-completion races without executing containers.
Host and Research VM suites exercise policy integrations, not real vendor
startup/model/tool acceptance. Keep pending acceptance explicit in the catalog.

For explicitly authorized focused builds, select the actual exported check:

```bash
nix build --no-link -L .#checks.x86_64-linux.vm-integration
nix build --no-link -L .#checks.x86_64-linux.research-integration
nix build .#installer-iso
```

## GitHub routing

`.github/ci-paths.json` and `scripts/ci_changes.py` own the ordered routing policy.
Component paths take precedence over Markdown suffixes. Include deleted paths
and both rename sides from NUL-delimited Git output.

Automatic PR/push checks always cover routing/whitespace, then affected static
and Rust checks. Unknown paths, invalid lock data or missing history select all
fast checks. They do not select host evaluation, closure preparation, runtime
builds or VMs. DCO and secret scanning are separate.

Use `workflow_dispatch` for required build/runtime evidence, selecting the
revision and optional full 40-character comparison `base_sha`. Empty, malformed
or unavailable comparison history selects the full inventory. With usable
history, affected package/worker-image, installed-onboarding, generated-flake,
configuration and VM gates are added according to that same policy. Shared
security paths select their combined gates; a Rust-only change need not build
Research. Non-VM package builds run in groups of at most four; Research closure
preparation and configuration projections remain sequential.

Docs-only routing requires no Nix installation. Fast checks consume contributor
tool caches; explicit builds may upload build cache. Optional cache setup has a
three-minute timeout and its failure never skips the checks. Dispatch has a
separate concurrency group so a push does not cancel requested verification.
Before merge, obtain the required evidence for this revision; a green fast run
or skipped expensive gate is not runtime acceptance.

## Packaging and larger workloads

`lib/mkRustPackage.nix` includes root Cargo manifests/lock, all workspace
manifests and only selected crate source. Root workspace versions and crate
descriptions feed Nix packages; worker image metadata supplies its default
reference. Add required root/path inputs explicitly. Unrelated source changes
should preserve other package derivations; shared manifests/toolchain changes
still invalidate caches. `checks.*.rust-package-sources` guards these boundaries.

Full system/ISO/VM work needs explicit workload scope. Installer VM state lives
below `/var/tmp/tentaflake-e2e-<user>/`; no host block device is passed through.
Keep builds and restore fixtures separate from production deployment.

Cargo/Nix remain the build definition. Bazel would add another target/dependency
graph without replacing NixOS/VM work; no measured speedup justifies that here.
Reconsider only with a bounded measurement demonstrating a relevant bottleneck.
