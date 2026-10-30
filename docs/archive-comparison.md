# Selective archive comparison

Historical review summary (2026-10-01/02), not current support or verification
of this checkout. Current policy is in [adapters](agent-adapters.md),
[the threat model](15-threat-model.md) and [build guidance](17-builds.md).

The review compared current base `38abc6c`, archive base `d62d18e` and their
working trees, with common ancestor `4626e46`. The archive had uncommitted
candidate work; it was read as reference, not adopted as a replacement base.
Existing TLS/dependency pins, Rust source isolation, bounded recovery and
quota-aware backup were preserved; neither lockfile was replaced wholesale.

## Retained lessons

- Remove deployment-specific Editor, Hive Research and Piper integrations from
  the generic template; keep consumer-owned integrations outside core.
- Compare exact live OCI mounts, tmpfs, ports, resources and internal networks,
  with bounded inspection and explicit unknown evidence. Real Podman/runsc
  inspection exposed missing AppArmor/capability evidence that synthetic complete
  records had hidden; the fixture was not a fully confined Podman acceptance.
- Restrict a broker's writable runtime directory to its exact private child.
  An empty systemd capability list omitted the directive; an empty string cleared
  it. Inspect actual write denial/capabilities, not only declaration values.
- Port worker lifecycle and crash/capacity tests together; do not transplant an
  isolated descriptor fix into a queue with different ownership semantics.
- Keep streaming failures, audit persistence, conservative budgets and replay
  prevention together. API/schema reuse does not establish vendor compatibility.
- No Sui/Move, attestation or ZK integration was adopted. Remote deployment and
  credential automation require separate review of SSH, secrets and disk authority.

## Historical evidence limits

That review recorded passing source/unit/evaluation/build and corrected VM
fixtures, including credential write denial, Docker drift, recovery and quota
restore. A later bounded-output regression covered stdout/stderr overflow.
OSV found a Dev Container dependency advisory; the retained dependency update
was checked with package/help and local FTP fixtures before scans passed.
Full Dev Container startup was not exercised because the host lacked an OCI
backend. No host activation, production deployment, real controller-provider
acceptance or production FTPS claim followed from those checks.

These are past observations, not checks rerun for the current changes. Git
history retains the detailed review record. Current worker, broker, generic
adapter and onboarding contracts supersede the archive's follow-up proposals;
do not restore old wrappers or runtime-prefixed identities from historical code.
