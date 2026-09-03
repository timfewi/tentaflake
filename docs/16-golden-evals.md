# Golden host-policy evaluations

Tentaflake's Golden Eval Set is a small, versioned set of worker requests and
expected host-policy outcomes. It checks the boundary enforced by the host. It
does not score an AI model, judge answer quality, solve prompt injection, or
turn an attestation into authorization.

The corpus is `tests/golden-evals.json`. The fast schema check loads it with
`tests/golden-eval-runner.py` and runs the runner's Python unit tests. The
NixOS VM integration test additionally copies both files into a test machine
and exercises the real worker, systemd path activation, Docker, and gVisor
fixture.

## Terms used here

- **Corpus:** the complete JSON file containing the version and test cases.
- **Case:** one request plus its expected status, artifact projection, and
  ordered audit transition.
- **Action class:** the policy category assigned to a request, such as
  `local-reversible`, `financial`, or `forbidden`.
- **Pending:** private host state holding a request that needs an operator
  decision. The agent cannot approve its own request.
- **Capsule:** the short-lived gVisor container used to execute allowed work.
- **Audit event:** one JSON line recording a host-side policy transition.

See [Disposable execution and approval](13-disposable-worker.md) for the full
worker lifecycle.

## What v1 checks

The checked-in corpus currently has one case for every action class. The schema
requires at least one case for each class and allows additional cases with
unique IDs.

| Action class | Expected transition |
|---|---|
| `local-reversible` | accepted, then completed in a capsule whose network namespace exposes only loopback |
| `external-reversible`, `irreversible`, `financial`, `production`, `communicative` | held privately, then approved and completed |
| `privileged` | held privately, then denied |
| `forbidden` | accepted by the inbox parser, then rejected without an artifact |

For every corpus case, the runner checks:

- the exact result status and artifact projection;
- the exact ordered audit-event sequence, with no missing, duplicate, reordered,
  or additional event;
- that approval-required work has no result artifact before the operator
  decision;
- that a consumed approval cannot be reused and returns the precise
  missing-pending-job error;
- that the capsule has no Docker socket and an offline case exposes only the
  `lo` network interface; and
- that the fixture's sensitive marker never appears in the worker audit log.

The runner also exercises several bounded-runtime conditions:

- It places 1,024 non-regular inbox entries beside one valid request. It
  observes that the valid request completes, `NRestarts` increases, the cursor
  file eventually disappears, the junk remains untouched, the worker becomes
  inactive, and the audit stops growing.
- It stops the watcher, pre-fills bursts that independently exceed the private
  pending count and byte limits, restarts both watcher and worker explicitly,
  and observes the excess request being rejected. This avoids depending on a
  directory-change event for files that already existed before the watcher.
- It races two `deny` commands for one pending request and requires exactly one
  successful claimant and one terminal denial.
- It measures the exact queued request size and reproduces the runtime byte
  reservation, including the unused `max_pending_bytes` allowance. The fixture
  leaves enough space for the formula with that allowance removed but less
  than the complete reservation, then requires the capacity-specific rejected
  result and audit event.

These observations have deliberate limits. The cursor test does not inspect
the cursor's owner and cannot prove from outside the process that no unbounded
allocation occurred. The state-capacity test does not trace OCI runtime calls,
so by itself it does not independently prove that a capsule `create` or
`start` call never happened. Source review and focused Rust tests remain
separate evidence for those implementation-ordering claims.

The corpus also does not prove that every upstream Hermes or ZeroClaw tool
routes work through the worker, that a production host matches the VM, or that
model output is safe. Those remain deployment and threat-model concerns; see
the [threat model](15-threat-model.md).

## Fast schema and oracle check

This check validates the strict JSON fields and types, complete action-class
coverage, policy projections, ordered audit oracle, duplicate approval error,
and loopback-only offline command without booting a VM.

Prerequisites:

- Nix with the `nix-command` and `flakes` features enabled.
- Access to the pinned inputs in `flake.lock`, either from the local Nix store,
  a configured binary cache, or normal network access.

Run:

~~~bash
just golden-eval-schema
# equivalent:
nix build .#checks.x86_64-linux.golden-eval-schema -L
~~~

Once the pinned inputs are available, this normally takes seconds. On an
uncached build, the relevant log ends with output similar to:

~~~text
Ran 13 tests in ...
OK
~~~

A substituted cached result may finish without replaying the test log.

## Full VM check

The VM check is the runtime evidence gate.

Additional prerequisites:

- an x86_64 Linux host;
- KVM available to the Nix builder, normally through `/dev/kvm`; and
- `kvm` listed in the Nix builder's supported system features.

Run:

~~~bash
just golden-evals
# equivalent:
nix build .#checks.x86_64-linux.vm-integration -L
~~~

A non-cached run normally takes several minutes, depending on CPU speed and the
Nix cache. CI allows up to 45 minutes. The VM log includes this success line:

~~~text
tentaflake-agent-host-policy v1: 8 cases passed
~~~

The check boots a disposable test VM. It does not activate the contributor's
host, contact a public Golden Eval endpoint, or publish an agent action. A
cached result may complete without showing the internal VM line again.

## Troubleshooting

| Symptom | Meaning and next step |
|---|---|
| Nix reports that the `kvm` system feature is missing | The fast schema check can still run. For the VM check, verify that the builder can access `/dev/kvm` and advertises `kvm`; do not report runtime verification from the schema check alone. |
| Nix cannot fetch a locked input | The required source is absent from the local store/cache. Restore normal access to the pinned input or run the check in the project CI; do not replace it with an unpinned dependency. |
| `fields mismatch`, `must be a boolean`, or `expected policy projection` | The corpus no longer matches schema v1. Fix the case or intentionally version the schema, runner, unit tests, and this page together. |
| `audit transition mismatch` | Compare the printed `expected` and `observed` lists. Reordering, duplication, and unexpected terminal events are security-policy changes, not snapshots to accept casually. |
| The VM build fails after boot | Keep the `-L` output and inspect the derivation log with `nix log .#checks.x86_64-linux.vm-integration`. The failing subtest identifies whether the problem is worker policy, systemd, Docker, or gVisor. |

## Evolving the corpus

Keep the corpus generic and free of real endpoints, credentials, prompts, or
deployment identities.

- Use a stable, unique lowercase ID.
- Keep at least one case for every action class; multiple cases for one class
  are allowed.
- Add a deterministic policy transition rather than a timing snapshot.
- Keep `expected_status`, `artifacts_available`, and `expected_events`
  aligned with the schema's exact policy projection.
- Treat the schema as closed: adding or renaming a field requires coordinated
  runner, unit-test, and documentation changes.
- Treat a changed expected result as a security-policy change: add a focused
  test, update the threat model where needed, and record it in the changelog.
