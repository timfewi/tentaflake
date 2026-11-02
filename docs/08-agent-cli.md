# Declarative agent configuration

Use `tentaflake.toml` as the only public instance-definition source, with non-secret
schema-v1 data. Runtime-native settings belong to the selected adapter; broker,
worker, quota and Research enforcement stay in Nix host modules. The public
Research section maps explicit relay permissions into that existing policy.
Host policy may consume instance metadata, but must not define additional instances.

The public repository keeps a commented, installation-independent
`tentaflake.toml` with only these active values. It is a starting template, not
the fallback configuration for an existing installation:

```toml
schemaVersion = 1
agents = []
```

There is no separate example inventory. No agent is declared or started by this
default. Existing `agents.toml` or `agents.json` requires conscious
[migration](agent-adapters.md#breaking-migration), even if TOML also exists;
it must never be silently ignored as an empty host.

## Public host services

The same `tentaflake.toml` can explicitly enable the implemented local host API,
connector front door and workload identity observer. These sections project
through the shared descriptor into `tentaflake.hostApi`,
`tentaflake.integrations` and `tentaflake.workloadMapping`; they do not define
another agent inventory. For an already declared `assistant`:

```toml
[hostApi]
enable = true
allowActions = false
allowLogs = true
peers = [{ uid = 10001, agents = ["agent-assistant"], permissions = ["read", "logs"] }]

[connectors]
enable = true
configFile = "/run/tentaflake/integrations.json"
advisory = true

[workloadMapping]
enable = true
[workloadMapping.bindings.agent-assistant]
tenant_id = "example"
workload_id = "assistant"
policy_revision = "review-1"
```

All three services default off. Service fields beyond `enable` require explicit
`enable = true`. Host API enablement requires `peers`, including `[]` to deny
everyone; it never selects the legacy implicit group grants. At most 64 unique
peer UIDs are allowed, in `1..4294967294` (never root). Each peer has at most 128
unique, exact declared `agent-<name>` IDs and unique finite permissions.
Accepted operations are `read`, `actions`, `logs`, `propose`, `tool_requests`,
`tool_decisions` and `host_health`. Actions, logs and worker operations require
their corresponding `allowActions`, `allowLogs`, `allowToolRequests` and
`allowToolDecisions` switches. Decisions also require request-read authority
both globally and on that peer. Empty agent scope permits only `host_health`
(or no permissions), not host-wide agent grants. Grants do not provision worker
queues or otherwise make unavailable agent capabilities available.

Connector enablement requires an explicit runtime JSON path strictly beneath
`/run/`, at most 256 ASCII characters, without empty, dot or traversal segments.
The operator provisions the root-owned runtime configuration and credentials
separately; TOML accepts no credential contents, keys or provider configuration.
`advisory` selects evidence-linked review storage, not an autonomous observer,
model execution or remediation authority.

Workload bindings are policy for existing runnable declarations, not declarations
themselves. At most 64 bindings are accepted, only with Docker; stopped Pi Durable and
OpenClaw adapters cannot be bound. Tenant/workload pairs must be unique, and all
three identifiers are bounded to 64 ASCII letters, digits, underscores, hyphens
and dots (a dot cannot be first). Unit names, engine endpoints, commands and
arbitrary paths cannot be supplied. Runtime provenance checks and explicit
generation-authority initialization remain required.

`ui`, `board`, `monitoring`, `securityObserver` and `llm` remain disabled-only
until their public schema has implemented runtime backing.
Pi Durable `run_read`, `run_submit` and `run_stop` grants remain rejected. The public
compiler currently receives only `pkgs` and `lib`, while the pinned Pi Durable source,
runtime package and verified image bundle are flake-local artifacts; no trusted
artifact-selection input reaches this compiler. Public Pi Durable entries therefore
remain stopped/unreviewed, without accepting user-supplied packages, image
selectors or Nix expressions to bypass admission.

## Public Research permissions

For an already declared Hermes or ZeroClaw agent, opt in to the existing isolated
[Research service](16-research.md), with a dedicated non-root host relay UID:

```toml
[research]
enable = true

[research.agents.agent-assistant]
uid = 62101
```

Enablement requires 1–128 exact declared `agent-<name>` IDs with distinct UIDs in
`1..4294967294`. Each entry accepts only `uid`. Unknown IDs, wildcard scopes,
duplicate UIDs, the `dev` profile, stopped Pi Durable/OpenClaw adapters and generic OCI
without an accepted Research integration are rejected by both validators.
Adapter support comes from the generated runtime catalog; the host module also
checks the actual runnable adapter and exact read-only client/socket mounts.
Agent names such as `researcher` are labels, never capability grants.

`enable = true` projects to `services.secureResearch.enable`; `agents` projects
intact to `tentaflake.research.agents`. Omission, an empty section and
`enable = false` emit no Research settings and preserve independent consumer
policy; other fields require explicit enablement. These grants do not enable
automatic agent startup, native web tools, unrestricted networking or models.

Operators must separately configure distinct service/egress UIDs, the VPN,
public resolvers, firewall and fresh readiness observations. Missing service
identities or VPN/resolver configuration fails host evaluation; missing/stale
readiness refuses public requests. A disabled isolated service, host UID
collision, legacy fetch broker or missing mounts also fails closed. Credentials,
provider configuration, paid/data grants and VPN controls are not accepted in
this section. Optional search/browser/OCR capabilities remain separately
configured host policy. Offline schema validation proves only the declaration,
not deployment readiness or vendor acceptance.

## Public ordinary backups

The optional `storage` section configures the existing encrypted Restic backup
module, not disk provisioning, relocation or a coordinated recovery checkpoint:

```toml
[storage]
enable = true
paths = ["/var/lib/tentaflake/agents/assistant", "/var/lib/tentaflake-broker-llm-agent-assistant"]
repositoryFile = "/run/agenix/restic-repository"
passwordFile = "/run/agenix/restic-password"
# environmentFile = "/run/agenix/restic-environment"
```

It defaults off; omission, an empty section and `enable = false` remain valid.
These disabled forms emit no backup settings, preserving pre-existing consumer
Nix backup policy; `enable = false` is not a command to disable that policy.
Any other storage fields require explicit `enable = true`. Enablement requires
1–128 unique paths plus both runtime file references. Optional `environmentFile`
references backend credentials; no repository URL, password, credential contents,
Nix expressions, initialization, reset or relocation controls are accepted.
References must be strictly below `/run/`, at most 256 ASCII characters, with
nonempty safe segments and no dot/traversal segments. Operators provision and
protect these files separately and prepare the repository out of band.

Public backup paths are restricted to `/var/lib/tentaflake/agents/<name>`,
that directory's `/workspace`, or `/var/lib/tentaflake-broker-llm-agent-<name>`,
with the usual 1–57-character agent-name grammar. An exact explicitly declared
agent `stateDir`, such as `/var/lib/private/kept`, is also accepted; other
arbitrary host paths and descendants of custom paths are not. Paths are at most
256 ASCII characters; empty, dot, traversal, repeated/trailing slash and
container-engine state paths are rejected. Selecting a custom `stateDir` does
not imply that its old default directory contains the agent's state: select the
effective directory explicitly. Managed quota mounts inside selected trees are
included separately by the existing module.

Fields project directly to `tentaflake.backup`; its timer, retention and integrity
check policy still applies. Existing low-level consumer Nix backup settings stay
independent and remain necessary for other reviewed host paths or policy tuning.
This does not claim complete storage coverage, create a repository, move state,
restore data or reconcile budgets and receipts. Ordinary live file backups are
**not cross-service-consistent checkpoints**; see
[backup and restore limitations](07-operations.md#backup-and-restore).

## Add an agent on an installed host

```bash
tentaflake runtimes --json
tentaflake agent template hermes assistant > incoming.toml
tentaflake agent validate incoming.toml
tentaflake agent plan incoming.toml --json
tentaflake agent import incoming.toml
```

Fixed preset templates and reviewed digest-pinned generic OCI workloads are
supported. Generic argv executes only inside the offline capsule:

```bash
tentaflake agent template generic coding \
  --image "$REVIEWED_AGENT_IMAGE" -- agent-command --workspace /workspace \
  > incoming.toml
tentaflake agent plan incoming.toml
tentaflake agent import incoming.toml
```

Generic workloads have no accepted provider/model or Research integration.
Arbitrary host commands, mounts and credential inputs remain forbidden.

Templates require no host configuration. Validation/plans/import require the
generated CLI inputs and installed pinned Nix inputs. Evaluation is offline,
bounded to 60 seconds and bounded output; it does not download images. Incoming
files are limited to 1 MiB and 128 stopped additions. Alternate host mounts,
runtime settings and credential paths are not public agent inputs.

Import atomically adds to the same live `tentaflake.toml` schema-v1 document,
preserving existing optional `_securityNote`. It rejects legacy arrays and duplicate
names/identities across TOML, active inventory and host declarations, including
unmanaged OCI containers.
Management uses the generated `configuration_name` selector for rebuild and
installed-host validation, while `host_name` remains the display hostname.
Changing TOML `[system].hostName` does not rename the flake output.
Existing inventory comments and formatting are retained; comments and root
metadata from incoming workload fragments do not replace host metadata.
Inputs/destination must be regular files. Failure preserves existing bytes;
imports share a lock and check for changes before publication. Administrators
can still edit after that
check. A leftover `tentaflake.toml.pending` is refused, not overwritten.

`--json` and `--hide` apply to validate/plan/import. Hidden plans redact names,
paths, image references and errors. Raw Nix source diagnostics are not printed.
Plans prove source validation only, not vendor acceptance or live readiness.
Scaffolds remain non-runnable, and secure startup gates still apply.

Import does not stage Git files, rebuild, activate or start anything. Review and
track inputs before a deliberate host update: Git-backed flakes only include
tracked sources. A failed update does not replace the active generation.

## TOML definitions

To declare stopped instances, replace the empty inventory with reviewed entries
(do not retain `agents = []` alongside `[[agents]]`):

```toml
schemaVersion = 1

[[agents]]
adapter = "hermes"
name = "assistant"
autoStart = false

[[agents]]
adapter = "pi-durable"
name = "experiment"
autoStart = false
```

Both root fields are required. `_securityNote` is optional string metadata; unknown
root/entry fields, legacy runtime arrays, unsupported versions and malformed
arguments fail. Adapter arguments are subject to the shared validation. TOML
date/time values and non-finite floats (`nan`, `inf`) are unsupported; use the
JSON-compatible value types required by the shared semantic schema.
Never include literal credentials: TOML, settings, environment values and seed
paths become Nix-store inputs. [Secrets](04-agenix-secrets.md) belong in runtime
files outside the capsule.

Names are globally unique across adapters; the IDs above are `agent-assistant`
and `agent-experiment`. Host state defaults to `/var/lib/tentaflake/agents/<name>`.
The examples above are documentation only, not an installed inventory. Public
agent fields are limited to `name`, `adapter`, `autoStart`, `stateDir`, `user`,
`group`, `uid`, `gid`, `containerUid` and `containerGid`; generic agents also
accept a validated `definition` containing their reviewed image and in-capsule
argv. Native settings, host commands, extra mounts and environment overrides
are refused rather than interpreted.
Examples are not turnkey integrations or evidence of vendor acceptance.

An explicit `stateDir` preserves operator-selected existing state (for example,
`/var/lib/private/kept`); omission uses `/var/lib/tentaflake/agents/<name>`.
The shared Rust/Nix public schema accepts normalized `/var/lib` paths except
`/var/lib/docker`, `/var/lib/containers` and their descendants, even for stopped
agents. Prefix lookalikes such as `/var/lib/docker-agent` remain accepted.
Validation and migration refuse reserved engine paths; they never relocate
data. Excluding known engine state does not prove arbitrary other host paths
safe. Operators remain responsible for dedicated state selection; low-level
trusted Nix configuration is a separate authority boundary.

### Synthetic researcher and legal example

This separate documentation-only example declares exactly one `researcher` and
one `legal` agent, both stopped for automatic startup, using the existing Hermes
compatibility preset. It does not replace the checked-in empty `tentaflake.toml`
or create a second installed inventory:

```toml
schemaVersion = 1
_securityNote = "Synthetic stopped example only; no credentials or runtime acceptance."

[[agents]]
adapter = "hermes"
name = "researcher"
autoStart = false

[[agents]]
adapter = "hermes"
name = "legal"
autoStart = false
```

The exact IDs are `agent-researcher` and `agent-legal`. These names are role
labels only: they grant no Research, model, legal-advisory, connector or host API
authority. The example contains no credentials, credential paths, provider
settings or host-service grants, and uses no new schema fields or admission
override. Hermes vendor acceptance remains pending; this is not an admitted Pi Durable
runtime or evidence that either role can perform live work.

To review it separately, save only the TOML block as `tentaflake.toml` in a
private draft directory without legacy inventory siblings, then use the offline
`tentaflake config validate` command described below. Validation does not build,
activate, start agents or provision credentials. Any future live integration
still requires reviewed host broker, Research, worker and quota policy and
explicit operator authorization.

## Installation-owned configuration

Keep the public template empty. Maintain real desired state in a separate
consumer flake (recommended) or in your deployment fork; `tentaflake.toml` is the
public **format**, not a requirement to publish your deployment data. A private
Git repository does not make TOML secret: evaluated source enters the Nix store.
Provider keys, passwords, private memories and account data must use runtime
provisioning outside TOML and Nix inputs.

### Consumer flake (recommended)

Copy [the consumer example](../examples/consumer-flake.nix) to your own
`flake.nix`, and copy the public commented `tentaflake.toml` to the **same consumer
directory**. Supply the example's local hardware, host-policy, Home Manager and
encrypted-secret modules, then edit only your consumer's desired state:

```text
my-installation/
  flake.nix
  flake.lock
  tentaflake.toml
  machines/agent-box/{default,hardware-configuration,host-policy}.nix
  home.nix
  secrets/secrets.nix
```

The consumer pins Tentaflake as an input and imports
`tentaflake.nixosModules.default`. It uses the input's compiler on its **own**
source file, as the example does:

```nix
(tentaflake.lib.${system}.systemFromData {
  file = ./tentaflake.toml;
})
```

Paths are relative to the expression containing them. When that expression lives
in your consumer `flake.nix`, `./tentaflake.toml` is consumer-owned, not the empty
file inside the Tentaflake input. Do not import the input's `configuration.nix`,
reuse its `nixosConfigurations` as your installed host, or fall back to
`"${tentaflake}/tentaflake.toml"` when your source is missing.

`systemFromData` remains **file-only** and returns one module; wrap it in
`imports = [ (systemFromData { file = ./tentaflake.toml; }) ];` when using a
consumer NixOS module, passing the helper through `specialArgs`. There is no
inline `data`, injectable builder or implicit source search. `mkAgent` and
adapter builds remain internal.

After creating and reviewing the files in your consumer repository:

```bash
git add flake.nix tentaflake.toml # Include the referenced local modules too.
nix flake lock                  # Review and retain the exact input pins.
git add flake.lock
tentaflake config validate ./tentaflake.toml
nix eval --json .#nixosConfigurations.agent-box.config.tentaflake.agentInstances
nix build --no-link .#nixosConfigurations.agent-box.config.system.build.toplevel
```

These commands do not activate a host. The flake selector remains `agent-box`
(`tentaflake.configurationName`) even if TOML changes `[system].hostName`.
One root `tentaflake.toml` is one desired system; use separate consumer roots for
independently managed systems rather than accidentally applying one host's
inventory to another.
The example explicitly projects `tentaflake.stateVersion` into
`system.stateVersion`, as the fork/installer root configuration does. Keep an
existing installation's state version; updating an input is not permission to
raise that compatibility setting or substitute NixOS's current-release default.

The configuration-free `config validate|normalize|migrate` commands can operate
on this consumer file directly. Installed management is narrower: generated CLI
inputs currently target `/etc/nixos`, and `agent plan|import` additionally expects
the installer's local `lib/agentPlanEntry.nix` layout. A pure input-based consumer
does not acquire that layout by importing modules. Edit/validate its TOML and
evaluate the consumer flake explicitly; do not point management at the upstream
template or claim that it automatically locates an arbitrary consumer checkout.

### Deployment fork and installer-owned source

In a deployment fork, the fork's root `configuration.nix` already loads the
adjacent `tentaflake.toml`. Keep real values on the deployment branch/repository,
review them with the same validation/evaluation/build steps, and do not contribute
them back to the public template. Merge upstream code while deliberately
preserving that deployment file and its pinned inputs. Existing inventories
require the explicit [migration](agent-adapters.md#breaking-migration), never
replacement with an empty template.

The installer creates a separate Git repository at `/etc/nixos` with its own
root TOML and local compiler/modules. Its one-time fresh-install initialization
creates an empty inventory only if absent, preserves an existing regular file
byte-for-byte and refuses legacy or unsafe paths. Later evaluation and CLI
onboarding require the local file; deleting it is an error, not reinitialization.

### Git filtering and missing sources

Ordinary Git flake references (`.` or a local repository path) include tracked
working-tree files, including dirty tracked edits, but exclude untracked and
ignored files. Track `tentaflake.toml` and every referenced module in the owning
consumer/fork; `git add` makes a reviewed new file visible without requiring a
commit. Commit the reviewed files and lock together for reproducible publication.
Do not add the real TOML to `.gitignore` or rely on Git index-hiding flags to keep
deployment changes out of the public repository: use a separate repository or
deployment branch instead.

`systemFromData` refuses a source missing from the evaluated Git snapshot,
including an on-disk untracked/ignored TOML. It never substitutes the upstream
empty file. An explicitly tracked `agents = []` is a deliberate empty system;
a missing file is not. Do not bypass a missing-file refusal with `path:.` or a
broad source copy: that changes Git filtering and can include private files in
the Nix store. Git filtering is not a secret-management boundary.

Offline `config validate` reads the selected local file and can therefore pass
before Git tracking is fixed. Validate **and** evaluate the actual consumer
flake. Legacy-sibling checks in Nix cover its filtered source; local CLI checks
also see local sibling files. See [build boundaries](17-builds.md) for the
real-Git source and generated-consumer regressions.

## Offline desired-system commands

`tentaflake config validate`, `tentaflake config normalize` and
`tentaflake config migrate` operate offline, without host configuration or Nix
evaluation. See their `--help` for input/output arguments. The versioned
`schema/system-v1.json` descriptor defines accepted sections, constraints and
Nix projections. Unknown fields and unsupported category enables fail closed.
The `system`, `network`, `sandbox`, `shell` and `container` sections have Nix
projections, as do the implemented `hostApi`, `connectors`, `workloadMapping`,
Research relay `research` and ordinary-backup `storage` contracts above.
`llm`, `ui`, `board`, `monitoring` and `securityObserver` accept only
`enable = false`; `true` is explicitly rejected. These reserved sections do not
implement those integrations.

`config validate` and `config normalize` require the input basename
`tentaflake.toml`; a private draft directory is supported. Both refuse sibling
`agents.toml`, `agents.json` or `my-agents.nix` files.
Normalization uses the same semantic validation as the compiler and normalizes
only supplied data, without inserting defaults.
`config normalize --json` emits the normalized data itself.
`config validate --json` emits `{"schemaVersion":1,"valid":true}` on success;
invalid configuration emits
`{"schemaVersion":1,"valid":false,"error":"invalid_configuration"}` with exit
status 2. See command help for positional input/output arguments.

Migration retains its source and creates its destination only if absent. It
refuses unsupported behavior rather than silently dropping it; keep the original
and a protected backup until the candidate has been reviewed. Never substitute
an empty configuration when a desired-system file is missing.

## Apply deliberately

Evaluate and build the complete selected host, review the diff/assertions, and
activate only with operator authorization. Balanced automatic startup requires
the same ID's broker, worker, quota and Research declarations. `autoStart = false`
does not prevent activation from creating declared worker services or quota
filesystems. OpenClaw, and Pi Durable without explicit host admission, create only
refusal units, not those runtime resources.

After authorized activation, check `tentaflake status`, `tentaflake doctor
--security`, the exact inventory unit and actual application behavior. For
example, a Docker instance uses `docker-agent-assistant.service`; Podman uses
`podman-`. A build or synthetic test does not prove live vendor acceptance.

## Remove an agent

Remove its definition and matching broker, Research, worker, quota and provenance
declarations; review backup paths and stop old writers deliberately. State,
quota images, budget state and external secrets are not automatic deletion
targets. See [operations](07-operations.md) and the
[breaking migration](agent-adapters.md#breaking-migration).
