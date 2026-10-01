# Selective archive comparison

Requirement: remove the Editor, Hive Research, and Piper TTS integrations;
compare the archived Tentaflake tree; adopt useful foundations on top of the
current changes without Sui.

## Compared evidence

Reviewed on 2026-10-01: current base `38abc6c`, archive base `d62d18e`, and
both working trees. The archive has 88 staged/untracked entries; its checked-in
HEAD alone does not describe the candidate changes. The latest shared commit
is `4626e46`: the current history has 21 subsequent commits, the archive seven.
The archive was only read; it is not a verified release or a replacement base.

The current tree's pre-existing configuration-free CLI help fix and regression
test are preserved. Current Rust/TLS dependency fixes, Dev Containers security
pins, Rust package source isolation, capped service recovery, and quota-aware
Restic backup/restore remain authoritative. Neither lockfile is replaced.

## Selected changes

| Requirement | Implementation | Verification |
|---|---|---|
| Remove unrelated integrations | Delete three optional modules, Piper voice package, flake exports, and current usage examples; document consumer migration | Flake output inspection, reference search, module evaluation |
| Detect actual runtime policy drift | Extend the generated manifest and CLI comparison to exact mounts, tmpfs, ports, resources, running state, and internal broker network | Rust positive/negative Docker and Podman cases, manifest evaluation, full CLI JSON/redaction test, live VM fixtures |
| Bound inspection | Capture both streams with a deadline, including inherited writers; probe broker health in batches of 16 | Output, large-stream, timeout, child-reaping, and ordered-probe tests |
| Bound broker resource use | Per-process memory/tasks/files and optional host declaration ceilings; count both enabled modes | Module boundary cases and VM systemd properties |
| Preserve scoped credentials through recovery | Exact private runtime directory under strict filesystem protection; preserve virtual key through unit restart/stop-start | Module assertions and VM write-denial, mode, token-stability, and health tests |

Archive code is adapted rather than copied as a whole. In particular, broker
limits retain the current `serviceRecovery.nix` policy. A malformed escaped
newline in an archived Rust regression fixture was corrected after its first
execution failed. Controller memory/tmpfs/CPU bounds accompany the exact
manifest representation and are documented as a compatibility change.

The first VM run exposed a further archive bug: the nested runtime directory
left its shared parent writable. The port now declares the parent read-only
and only the exact child writable. Generated unit inspection also showed that
`CapabilityBoundingSet = [ ];` omitted the directive; an empty string now
clears capabilities for credential and broker units. The VM checks actual
`CapBnd` and rejected writes rather than relying on option values alone.

A real Podman/runsc fixture also exposed a limit hidden by synthetic complete
Inspect records: the pinned backend reports an empty AppArmor profile and
`null` effective/bounding capabilities. The live doctor correctly reports the
missing AppArmor boundary as critical; that fixture is not evidence of a fully
confined Podman controller. Docker runtime tests additionally compare a running
fixture with its exact declaration and deliberately increase its memory limit.

The archive's independent inbox-descriptor fix belongs to its new cursor-based
worker queue. The current worker already opens independent scans through
`/proc/self/fd`; importing that fix alone would not improve this implementation.

## Useful follow-up work

| Archive change | Decision and prerequisite |
|---|---|
| Worker atomic claims, crash recovery, bounded inbox/pending queue, private state volumes | Highest-priority next unit of work. Port the lifecycle and its negative tests together; specify existing-state migration and preserve current quota-aware backup/restore and recovery. |
| Golden host-policy evaluations | Port together with the worker lifecycle they exercise. Schema tests alone would not prove the current worker implements the archive's queue/capacity behavior. |
| LLM streaming relay | Separate broker change; test partial streams, audit failure, disconnects, byte/time limits, and conservative budget accounting against the current TLS lock. |
| Generic OCI builder and versioned inventory | Valuable later foundation, but changes the declarative API and CLI ownership model. Preserve existing consumers and test legacy conversion before porting. |
| Worker MCP client and runtime defaults | Follow the inventory decision; verify the actual pinned Hermes/ZeroClaw images and their tool routing. |
| Remote setup/deploy/install and encrypted credential management | Separate operator workflows with larger SSH, secret, activation, and disk boundaries. The archive itself records missing end-to-end verification. |
| Reorganized agent skills | Move with the implemented APIs. Copying them now would document commands and options this tree does not have. |
| Sui/Move, attestation, ZK research | Excluded as requested; no code, checks, dependencies, or skills adopted. |

## Verification results

The baseline Cargo workspace passed before edits. A disposable baseline copy
then reproduced false-green memory drift: changing the default 2 GiB limit to
4 GiB still returned `Secure`. The selected CLI tests reject even a one-byte
drift. All 68 Cargo workspace tests, Rustfmt, Clippy, Nix lint/formatting, module
evaluation, generated installer-flake evaluation, and ShellCheck have passed.
The first VM run failed the credential write-denial check. The corrected VM
run passed, including write boundaries, empty capability bounds, key retention,
resource ceilings, crash recovery, backup/restore, and reboot. The extended
VM also passed the real Docker drift check and the negative Podman evidence
check. The final full flake gate passed against this corrected implementation.

Commands run successfully:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
deadnix --fail .
statix check .
nix fmt -- --ci
shellcheck installer/*.sh scripts/*.sh
./scripts/generated-flake-test.sh
nix flake check --offline path:<filtered-source-snapshot> -L
nix build --offline --no-link \
  path:<filtered-source-snapshot>#checks.x86_64-linux.vm-integration -L
```

The filtered source snapshot includes tracked and new source/test files and
excludes ignored files, without staging this work. Its implementation was
compared byte-for-byte with the worktree; only this final verification record
was updated afterward. Normal staged/committed checkouts can use
`nix flake check` directly. The explicit VM runs built and executed the tests;
the final flake gate reused those successful build results.

Flake output inspection confirms only `default`, `installer`, `observability`,
and `falco` module exports, with no Piper package. Current docs/examples were
searched for retired imports and installer editor variables. The final diff
preserves both lockfiles and the current recovery, backup, workspace-quota,
Rust packaging, and Dev Containers fixes.

No host activation or deployment was performed for these checks.
Real registry controller startup, upstream providers, and production
hosts remain outside the local fixtures' evidence.

The integration review on 2026-10-02 retained these changes on top of current
`main` and added an 8 MiB combined OCI output ceiling. A real subprocess
regression first reproduced oversized output being accepted, then verified
stdout, stderr, and combined overflow rejection. `just fast` passed with 74
workspace tests. Focused formatting, lint, policy, evaluation, and generated
flake checks also passed after updating the Dev Containers dependency patch.
The host VM passed in 206.75 seconds and the research VM in 36.54 seconds;
the packaged CLI and host toplevel built successfully.

The current OSV scan found `GHSA-c475-qrg2-pj4r` in `basic-ftp` 5.3.1.
The retained CLI release uses a pinned 6.2.1 Yarn resolution and updated cache
hash. Its Nix build, version/help commands, and a local `get-uri` FTP transfer
passed; OSV and Semgrep then passed. Full Dev Container startup was not run:
this host has no Docker/Podman executable. The focused transfer proves API
compatibility, not a production FTPS deployment.
