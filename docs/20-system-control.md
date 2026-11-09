# Approval-bound system control

`tentaflake-control` is an optional Rust Unix-socket service and fixed Unix client.
It owns drafts, approvals and build receipts, not the live desired source, host
activation, agents, board tasks or credentials. It is off by default. There is no
activation, arbitrary executable, Nix expression, command, flags, socket-selection
or host-control request. The ordinary schema CLI remains the semantic authority.

## Deployment boundary

The trusted components are the deployment-owned configuration, reviewed immutable
schema/compiler and optional build template, plus the single private state writer.
HTTP JSON and peer-supplied request IDs are untrusted. Exact Linux `SO_PEERCRED`
UIDs grant independent `read`, `propose`, `approve` and `build` roles. There is no
root/group bypass and no implied role inheritance. Human approval requires that
the deployment grant `approve` only to the human-facing authenticated adapter;
a direct UID grant is not evidence that a human actually reviewed a proposal.

System-control grants are separate from the host API's resource-scoped
`run_read`, `run_submit` and `run_stop` permissions. The
[Pi Durable runner mediator](19-web-ui.md#exact-scoped-pi-durable-runner-mediation) remains in the
trusted root host API; neither this controller nor the authenticated browser
bridge receives runner bearer files or the runner's numeric group. A successful
run, application readiness or saved board state is not system approval,
configuration authority or trusted host security evidence.

Enable `tentaflake.control` only after supplying its reserved non-root `uid`,
exact `peers` and reviewed `hashes`. The module defaults `package` and immutable
`normalizer` to the owning Rust packages; either may be explicitly overridden.
`minimumProfile` defaults to `balanced`, may be `strict`, and is enforced
on drafts, approvals and builds. An already-stricter base cannot be downgraded by
omitting the sandbox section. `dev` is not admitted through this service.

`desiredSource` defaults to `/var/lib/tentaflake/desired/tentaflake.toml`. Provision
it separately, readable but not writable by the service; it and its parent paths
must not be symlinks. Legacy sibling `agents.toml`, `agents.json` or
`my-agents.nix` causes refusal, not fallback. The service never writes this file.
Deployment settings are installed at `/etc/tentaflake-control.json`; that specific
configuration may resolve through the normal NixOS `/etc` link into an immutable
store file. Private state, sources and templates never follow symlinks.

State is `/var/lib/tentaflake-control` (0700), runtime is
`/run/tentaflake-control` (0711), socket `control.sock`. The socket allows Unix
connection at DAC level so exact configured UIDs need no broad supplementary
group; operations are always UID/role-authenticated before processing. Do not
assign the service UID to another application. Its private writer `flock`,
`openat`/`O_NOFOLLOW` file access, single-link ownership checks, fsynced temporary
receipts, atomic rename and directory fsync protect publication/recovery. A
persistence failure poisons the writer rather than allowing further dispatch.
There is no automatic state reset or receipt garbage collection.

The module removes capabilities, prevents privilege escalation, uses private
network/devices/tmp, read-only host/home protection, AF_UNIX only, and fixed CPU,
memory, task, descriptor and shutdown budgets. No sudo, credentials or broad
groups are configured. When builds are disabled the Nix daemon socket is hidden.

The sandbox keeps the live `/run` directory read-only, with only the service's
runtime directory writable. It does not bind `/run/current-system` or
`/run/booted-system` individually: binding an observation entry could hide a later
atomic activation replacement. The desired source likewise stays unpinned.

## Revision and approval contract

Transport is HTTP/1.1 over the fixed socket, JSON only, `Content-Length` required for POST (optional for an empty GET),
connection close, no transfer encoding, duplicate headers, nested duplicate JSON
keys, unknown request fields or arbitrary routes. Requests are at most 256 KiB,
headers 8 KiB, one connection is handled at a time (backlog 16), and the header/body
deadline is five seconds. Clients verify the configured service UID before sending
request data. The CLI's deployment path is fixed; browser data cannot select it.
Normalizer calls have a ten-second deadline and 256 KiB combined output ceiling.
Errors contain stable codes, never source snippets, subprocess stderr or secrets.
The validation route additionally returns only the normalizer's structured safe
diagnostic (fixed schema field paths/indexes, never source values or arbitrary
unknown field names). Unstructured subprocess output is not a diagnostic.

Canonical revisions are lowercase SHA-256 of the normalized JSON object serialized
without whitespace, with lexically ordered object keys. HTTP uses the shared
`schema/system-v1.json` descriptor and `schemaVersion = 1` configuration shape.
`schema_version = 1` identifies the surrounding control protocol.

Hash meanings:

* `compiler`: SHA-256 of the fixed normalizer executable bytes, verified against
  the deployment declaration before draft/approval/dispatch.
* `inputs`: SHA-256 of the reviewed template's exact `flake.lock` bytes when
  builds are enabled; otherwise the deployment's declared disabled-build input
  revision.
* `policy`: an effective SHA-256 over canonical JSON containing the declared
  policy revision, exact peer grants, minimum profile, source path, server UID,
  schema-byte digest and the complete fixed builder settings. Reusing a declared
  policy revision while changing these settings still invalidates approvals.

Read-role `GET /v1/system` returns:

```text
{
  schema_version: 1,
  revision,
  configuration,
  schema,
  hashes: {policy, compiler, inputs},
  capabilities: {propose: boolean, build: boolean, activate: false},
  observed: {current_system: string | null, booted_system: string | null}
}
```

`observed` is additive in protocol version 1. The controller reads the two fixed
link basenames under `/run` through a no-follow directory descriptor and bounded
`readlinkat`; it never follows their targets or accepts a request-selected path.
Each target must be one exact `/nix/store/<hash>-<name>` item (no child path).
Unreadable, missing, non-link, relative, malformed, oversized or non-UTF-8 targets
are independently `null`, without failing an otherwise available system response.
Activation can replace a link between requests; these are live, independent
observations, not an atomic pair or persisted desired-state fields.

Compare a reviewed built receipt's `output_path` with `current_system` to observe
which closure is selected now; `booted_system` may legitimately remain older until
a reboot. Neither a `built` receipt nor either observed path proves application
health, agent/model acceptance or successful source reconciliation. Health needs
separate host/application evidence. Observations do not enter desired revisions,
approval hashes or receipts, and confer no activation authority.

Read-role `GET /v1/system/source` returns exactly
`{schema_version: 1, revision, source, bytes}`. `source` is the configured desired
file's exact UTF-8 TOML, including comments, formatting and line endings; `bytes`
is its UTF-8 byte length. The revision is still the normalized-JSON hash, not a
source-byte hash. Both reads use the same fixed normalizer and source protections.

Read-role `POST /v1/system/validate` accepts exactly `{source}` and creates no
proposal, approval, request-ID receipt or build. A valid response is
`{schema_version: 1, valid: true, revision, configuration, changes}`; changes are
relative to the current desired configuration. Invalid TOML/schema/policy returns
HTTP 200 with `{schema_version: 1, valid: false, error: {code, message}}` and no
revision/configuration. Codes are `invalid_configuration` or
`sandbox_policy_denied`; the message identifies a safe field path where available.
Malformed requests, denied roles and unavailable source/compiler infrastructure
retain the ordinary 400/403/503 responses. Validation enforces the same minimum
and no-downgrade policy as proposals. The whole JSON request remains at most
256 KiB (escaping and the envelope count), not 256 KiB of TOML plus overhead.

`POST /v1/system/proposals` accepts exactly one of
`{request_id, base_revision, configuration}` or
`{request_id, base_revision, source}`. The configuration form serializes JSON as
TOML with `toml_edit` and requires semantic preservation. The source form stores
the submitted TOML bytes unchanged, including comments and formatting. Both
invoke `tentaflake config normalize <private>/tentaflake.toml --json` and durably
store candidate TOML, canonical JSON, base, hashes and typed changed paths.
A diff entry is `{path: "/sandbox/profile", kind: "added" | "removed" | "changed"}`;
there are no old/new secret excerpts. Proposals return in the `proposal` wrapper.
Supplying both representations or neither is invalid. Request-ID identity
includes the submitted representation and exact source string, so reusing an ID
with changed comments conflicts even when the normalized revision is unchanged.
Existing configuration-form request fingerprints remain compatible.

`POST /v1/system/proposals/{id}/approve` accepts exactly
`{request_id, base_revision, revision, hashes}` and returns `approval` with its ID,
proposal ID and the exact bound base/revision/hashes. Source, compiler, inputs or
policy drift invalidates the grant even if the candidate configuration is equal.

`POST /v1/system/builds` accepts exactly
`{request_id, proposal_id, approval_id, revision, hashes}`. An approval grants at
most one build intent. There is one builder and at most four outstanding intents.
The queued intent is fsynced before success, and `building` is fsynced before
execution. `GET /v1/system/builds/{id}` retrieves the receipt. Repeating an identical
request ID returns its existing result, including current build status; conflicting
reuse returns 409. IDs are 1–64 ASCII alphanumerics/underscore/hyphen. Capacity is
64 immutable proposals, 1024 operation IDs and a 32 MiB state snapshot. Exhaustion
fails closed; archival/retention is an explicit operator responsibility.

Receipts contain `{id, proposal_id, revision, state, output_path, error}`. States
are `queued`, `building`, `built`, `failed`, `unknown`. Restart converts every
abandoned queued/building receipt to `unknown` and never automatically dispatches
it. Existing terminal receipts remain retrievable. A lost response is resolved
with the same request ID, not a fresh build intent. The service must be stopped
before restoring its whole private state; restoring stale receipts can violate
operation history and is not an automatic recovery operation.

## Reviewed fixed builds

Builds additionally require `build.enable`, `daemonReady`, a sandboxed Nix daemon,
reviewed immutable `template`, `templateHash`, fixed `target`, and a deadline
(1–3600 seconds). No missing input has a builder fallback. The controller user must
remain an ordinary untrusted Nix client, not a Nix `trusted-user`. The operator must
supply daemon/build-worker resource and network policy: service cgroup limits do
**not** constrain independent daemon build workers. `daemonReady` is an explicit
operator prerequisite, not a fabricated probe of those external budgets. Client
timeout/termination does not prove that a shared daemon build has ceased.

The template must contain `flake.nix`, `flake.lock` and `tentaflake.toml`. It is a
reviewed root-owned, non-writable store directory, not an agent Git tree. Files and
all directory components are symlink-checked; dotfiles and `secrets`, `private`,
`runtime`, `state`, `target` and `node_modules` entries are rejected. Inventory is
bounded to 4096 entries, depth 32 and 32 MiB file content. No Git operation occurs.
The controller copies public template data into a new private build directory,
preserves executable bits and locked inputs, and replaces only `tentaflake.toml`.
The review must establish that permitted public filenames do not themselves carry
private data; filename filtering alone is not a secret detector.

The template fingerprint is exposed by Rust `template::fingerprint`: recursively
visit children in filename order; for each regular file hash its relative UTF-8
path length (u64 big-endian), path bytes, executable bits (u32 big-endian,
`mode & 0111`), content length (u64 big-endian) and content bytes, in order, using
SHA-256. Empty directories have no semantic fingerprint entry but are counted
against the inventory limit. Operators compute/review this before enabling builds.
The offline administrative command
`tentaflake-control template-fingerprint /nix/store/<reviewed-template>` prints
this exact fingerprint with the same paths, bounds, no-symlink and non-writable
checks. Offline computation requires one consistent owner instead of UID 0, so it
also works inside Nix build sandboxes that map immutable input ownership to
`nobody`. It does not attest deployment ownership: service validation and
materialization always independently require the real store's root UID.
The command needs no deployment configuration and exposes no server route or
browser-selected path.

The only build argv is:

```text
<fixed-nix> build --no-link --json --no-write-lock-file <private-candidate>#nixosConfigurations.<fixed-target>.config.system.build.toplevel
```

The module resolves the deployment-owned Nix package's `bin/nix` at build time:
some package versions expose a component symlink. The fixed settings contain its
real immutable store executable, not the symlink. Service admission still checks
every component without following links and requires root ownership and no write
bits; requests cannot select or replace the executable.

Child environments are cleared; only fixed `NIX_REMOTE=daemon`, private state
`HOME`/`XDG_CACHE_HOME`, and Nix settings retaining sandboxing and disabling fallback
are supplied. No inherited credentials, proxy settings, PATH override or client flags are forwarded. stdout and stderr are
jointly bounded; deadlines kill the child process group. A success must report
exactly one `outputs.out` with a single valid Nix-store item path, never an arbitrary
host path or executable. Failed/refused work changes neither desired source nor
active system. A returned output path is a build receipt, never activation or an
operator-ready rollout acceptance by itself. Human activation is outside this API.

## CLI and verification

The installed client provides `system`, `source`, `validate`, `propose`,
`approve <proposal-id>`, `build` and `status <build-id>`. POST operations read their
closed JSON body from stdin; GET operations read no input. Example flow is source →
validate → propose → inspect changed paths → approve exact hashes → build → status.
Service startup is `tentaflake-control serve <deployment-config>`; this
administrative argument is not exposed as a request route. Output is JSON and
any refusal exits nonzero. `validate` also exits nonzero for `valid: false`.
The fixed client preserves the additive `observed` object in `system` output;
it does not turn missing observations into a build, activation or health claim.

The repository's `just fast` gate includes the control crate's ordinary unit
tests. These cover Unix peer/role denial,
malformed/extra/duplicate fields, revision/policy/compiler drift, no sandbox
downgrade, stable IDs, receipt restart uncertainty, locks and symlinks, client UID
verification, fixed-client source/validation routes, bounded synthetic subprocesses,
store output paths and exact, independently nullable live-system observations.
Observation fixtures include malformed/oversized/UTF-8 targets, no-follow parent
and target checks, regular-file refusal and atomic link replacement. Synthetic
normalizers are labeled fixtures; the real compiler has its own shared corpus.
The explicit immutable fixture also provides
`offline_fingerprint_cli_preserves_runtime_digest_and_refusals`, which compares
the CLI digest with runtime validation and rejects linked inputs and extra arguments.

An additional **explicitly isolated** test exercises production deployment checks,
immutable store templates, the actual Unix protocol, fixed mock-builder argv,
success/failure/output/deadline bounds, exact source/lock preservation, and
queued/running restart uncertainty without any authorization/recovery bypass:

1. Build `crates/tentaflake-control/tests/fixture.nix` using the owning pinned
   `pkgs` (it only creates public synthetic store files, not a host build).
2. Also build the real `tentaflake-cli` package. In the owning pinned Rust
   environment set `TENTAFLAKE_CONTROL_FIXTURE` to the fixture store output,
   `TENTAFLAKE_REAL_NORMALIZER` to that CLI's immutable `bin/tentaflake`,
   `TENTAFLAKE_TEST_PYTHON` to the pinned Python executable and
   `TENTAFLAKE_UI_SOURCE` to the explicitly selected, reviewed UI checkout.
   Run `cargo test --locked --offline -p tentaflake-control --test immutable_unix -- --ignored`.
   This explicitly exercises the real normalizer and UI adapter against the real
   closed Rust transport, including proposal, approval, queued build and lookup.
   The builder remains synthetic; the UI contract test starts no build worker.
3. Run the `control-evaluation` module and package gates.

The immutable fixture tests are deliberately ignored by default because they need
explicit immutable artifacts; the UI variant additionally needs a selected UI
checkout. An ordinary fast pass does not claim they ran. CI runs
`immutable_unix_approval_build_and_recovery_contract` and
`real_source_validation_and_lossless_build_contract` with the built fixture and
CLI whenever control paths change. To run those two without a UI checkout, use
`--ignored --skip ui_adapter_speaks_real_closed_transport`. The UI source is a
test input only, never a production dependency of this OSS repository.
Real Nix-daemon/host output acceptance and all activation remain separate
operator tiers.
