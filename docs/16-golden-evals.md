# Golden host-policy evaluations

Tentaflake's Golden Eval Set is a versioned, deterministic contract for the
host-enforced agent boundary. It deliberately evaluates policy enforcement, not
model quality, model safety, or prompt-injection resistance.

The corpus is tests/golden-evals.json. The existing NixOS VM integration test
copies it into the test host and runs tests/golden-eval-runner.py against the
real worker, systemd path activation, Docker, and gVisor fixture.

## What v1 proves

Each action class has exactly one stable case:

| Action class | Expected transition |
|---|---|
| local-reversible | runs in the disposable offline capsule |
| external-reversible, irreversible, financial, production, communicative | remains private and has no artifact before a job-bound operator approval |
| privileged | remains private until a host operator denial produces denied |
| forbidden | never runs and produces rejected |

The runner also verifies stable result projections, expected audit events,
one-time approval consumption, absence of an artifact before approval, no
runtime socket in the capsule, and that a fixture sensitive marker never
appears in the worker audit log. After the corpus cases it places 1,024
persistent non-regular inbox entries beside a valid job and proves that the
worker completes a systemd restart (`NRestarts` increases), converges its
root-owned cursor checkpoint, runs that valid job without allocating an
unbounded candidate list or deleting the junk, and then remains inactive with a
stable audit despite the preserved entries. It also bursts one more
approval-required request than the VM fixture's private queue capacity and
proves the excess request is rejected while the accepted requests are cleaned
up through operator denial. A separate oversized-payload burst proves the
private byte limit independently of the count limit. Two simultaneous `deny`
commands then race for one approval-required request and must yield exactly one
terminal denial. Finally, the runner fills the fixed worker-state image until
its `statvfs` reservation is unavailable and proves that the worker publishes a
capacity rejection before it starts a capsule. Rust unit tests separately cover
cursor recovery after a worker reopen plus inbox mutation, malformed cursor
input, a stale cursor that resumes at EOF, pending count/byte admission, atomic
non-overwriting claims, outcome-unknown recovery, post-mount state-marker
refusal, capacity reservations, and bounded pending processing. It avoids
volatile timestamps, process IDs, latencies, and untrusted inbox names.

The v1 corpus supplements the existing VM tests for worker timeout, bad
requests, broker authentication, SSRF, direct-egress denial, and security
doctor findings. It does not prove that every upstream Hermes or ZeroClaw tool
routes work through the worker; that remains a deployment-specific residual
risk described in the [threat model](15-threat-model.md).

## Run it

The corpus runs as part of the VM integration check, which CI builds for every
non-Markdown source change:

~~~bash
just golden-evals
# equivalent:
nix build .#checks.x86_64-linux.vm-integration -L
~~~

This boots a test VM. It does not activate a contributor's host or publish an
agent action.

## Evolving the corpus

Keep the corpus generic and free of real endpoints, credentials, prompts, or
deployment identities.

- Use a stable lowercase id.
- Add a distinct, deterministic policy transition rather than a timing
  snapshot.
- State only stable status, artifacts_available, and audit-event projections.
- Update the runner validation and this document if the schema changes.
- Treat a changed expected result as a security-policy change: add a focused
  test, update the threat model where needed, and record it in the changelog.

The runner requires every v1 action class exactly once. A future schema version
may intentionally change that matrix, but it must preserve the same
fail-closed policy claim or document the migration.
