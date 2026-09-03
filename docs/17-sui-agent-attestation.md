# Sui agent attestation

The Sui agent-attestation package is an experimental, optional reference for
verifying an issuer-authorized statement over opaque Tentaflake evidence
commitments. Its source lives in
`integrations/sui-agent-attestation`. It is not a NixOS module, adds no default
networking or service, holds no wallet, and publishes nothing automatically.

The exact security claim is intentionally narrow:

> Given the exact audited package, Registry, AgentRecord, network domain,
> issuer key, protocol version, sequence policy, time window, and commitments
> that a consumer trusts, the chain can verify that the active issuer key
> signed the corresponding BCS payload and that the on-chain record was
> current when the verification executed.

The chain cannot prove that the issuer actually observed a host, that the
collector read correct state, that a digest names genuine evidence, that an
image is running, that an evaluation passed, or that an agent is safe, correct,
or harmless. A new sequence can contain stale or false facts if the issuer
signs them.

## Trust boundary

A balanced agent must not receive:

- Sui RPC access, a gas key, wallet, issuer key, `AdminCap`, or `UpgradeCap`;
- an arbitrary transaction signer or arbitrary transaction-payload relay;
- a Docker or Podman socket; or
- write control over the root-owned facts or evidence bundle that the issuer
  treats as authoritative.

The intended host implementation has three separate roles:

1. A fixed collector reads only declared, root-owned facts and produces a
   minimal, versioned, canonical evidence bundle.
2. A dedicated Ed25519 issuer reviews that bundle and signs exactly the
   documented BCS payload. Its network-specific key belongs in an HSM,
   threshold service, or equivalent runtime custody, never in Nix, Git,
   command-line arguments, a derivation, prompt, or agent workspace.
3. A distinct relayer has a small gas budget and may submit only the fixed
   `submit_attestation` transaction to an exact HTTPS RPC endpoint after
   chain-ID and finality checks.

These separations are off-chain controls. Move verifies a signature; it cannot
prove HSM use, honest collection, wallet budget, RPC policy, finality checks, or
separate operators.

The `AdminCap` ultimately controls registration, pause, revocation, and which
key is active. Transfer it to a reviewed multisig or compatible governance
path, separate from issuer and relayer custody. The capability has no `store`
ability, but the defining module deliberately exposes a controlled transfer
function; it is not “non-transferable.”

## Evidence, identity, and authorization are different

A secure consumer makes three independent decisions:

| Decision | Bound by this package | Still required from the consumer |
|---|---|---|
| Evidence statement | Active issuer signature over exact Registry/record IDs, sequence, issuer epoch, signed epoch window, and commitments | Canonical schemas and digest algorithms; a maximum acceptable attestation age; trust in collector and issuer behavior |
| Logical agent identity | Exact AgentRecord ID and its administrator-selected opaque handle commitment | A consumer-owned allowlist mapping that ID to the deployment identity it intends to trust |
| Authorization to act | Nothing about the transaction sender, relayer, or requested action | An actor/action capability, role, ownership check, or other explicit policy controlled by the consuming package |

A receiving package must keep expected Registry and AgentRecord IDs, minimum
sequence, maximum acceptable age in epochs, and commitments in its own
authenticated policy object. Passing caller-supplied values straight through as
“expected” values authenticates nothing.

The package's `verify_bound` function checks those values and returns a
non-storable `VerifiedAgent` for same-transaction composition. Both it and
`assert_verified_for` take `max_attestation_age_epochs` plus the current
`TxContext`; the maximum age is consumer policy, not a signed payload field. If
a proof is handed to another Move call, the receiver must immediately call
`assert_verified_for` using its own policy state and transaction context. It
must then require the separate actor/action authorization before changing
protected state.

Anyone can call `new_registry`, including an attacker. Therefore the universal
`VerifiedAgent` type alone cannot distinguish the intended Registry from an
attacker-created one. See the test-only pattern in
`integrations/sui-agent-attestation/tests/consumer_capability_example.move`,
which deliberately requires both a policy-bound proof and a separate
`ProtectedActionCap`.

## Object model

The package uses one shared Registry and one shared AgentRecord per logical
agent.

| Object | Ownership and purpose |
|---|---|
| Registry | Shared; one active issuer plus at most 15 staged keys, fixed V1 protocol, network-domain bytes, pause switch, maximum lifetime, and maximum backdating |
| AgentRecord | Shared; opaque handle commitment, active/revoked status, exact monotonic sequence, issuer epoch, signed not-before, exclusive expiry, and four 32-byte commitments |
| AdminCap | Address-owned and Registry-bound; no `store` ability; transferable only through the defining module |
| VerifiedAgent | Has `drop` but not `store`; carries the bound IDs, sequence, signed not-before, and commitments for same-transaction evidence checks, never actor authorization |

Sharing makes an object publicly addressable; it does not authorize mutation.
Administrative mutations require the Registry-bound `AdminCap`.
`submit_attestation` requires the active issuer signature. Rotation increments
the issuer-set epoch, so records signed under the prior epoch stop verifying
until re-attested. Revocation changes status instead of deleting the record.

The record stores only latest state; it has no unbounded object history. Typed
events now cover `AgentRegistered`, `AttestationSubmitted`,
`AdminCapTransferred`, `IssuerAdded`, `IssuerRotated`,
`RegistryPauseChanged`, and `AgentRevoked`. `AttestationSubmitted` carries both
the signed `not_before_epoch` and exclusive `expires_epoch`. It deliberately
omits the signature and four commitment bytes, so a production indexer must
join the event to the transaction and resulting object state when it needs the
complete statement. Events support monitoring; they do not authorize a caller
or prove that an administrative action was operationally well governed.

## Canonical signed payload

The issuer signs raw Ed25519 over exactly `BCS(SigningPayload)` in this order:

~~~text
SigningPayload {
  domain: vector<u8> = "TFA-SUI-ATTEST-v1",
  network_domain: vector<u8>,
  registry_id: sui::object::ID,
  agent_record_id: sui::object::ID,
  protocol_version: u64,
  issuer_key_id: u64,
  issuer_set_epoch: u64,
  sequence: u64,
  not_before_epoch: u64,
  expires_epoch: u64,
  policy_digest: vector<u8>,
  image_digest: vector<u8>,
  golden_eval_report_digest: vector<u8>,
  evidence_digest: vector<u8>
}
~~~

BCS encodes each `vector<u8>` with a ULEB128 length, each ID as its 32 address
bytes, and each `u64` little-endian. All four digest vectors must contain
exactly 32 bytes. This is raw Ed25519 message signing; it is not a Sui wallet
personal message, transaction intent, JSON representation, or
implementation-defined prehash.

The V1 fixture at
`integrations/sui-agent-attestation/test-vectors/signing-payload-v1.json`
pins field values, the 274 expected BCS bytes, public test key, and signature.
It contains deterministic public test material only and no private seed.
`scripts/verify-sui-attestation-vector.py` independently rebuilds the bytes
using only the Python standard library, verifies the signature, and rejects a
one-byte mutation.

Byte compatibility does not define evidence semantics. Before production, each
commitment needs a documented, versioned canonical input schema and digest
algorithm. For example, an “image digest” must say whether the 32 bytes are the
decoded OCI SHA-256 digest, a digest of a manifest envelope, or something else.
Without that definition, two honest implementations can sign different bytes
for the same human-readable claim.

## Sequence and replay boundary

On submission, V1 checks:

- current protocol and unpaused Registry;
- active AgentRecord belonging to that Registry;
- issuer-set epoch equal to the Registry's current epoch;
- sequence exactly equal to the stored sequence plus one;
- `expires_epoch > not_before_epoch`;
- lifetime no greater than the Registry maximum;
- not-before no later than the current epoch and no older than the configured
  backdating window;
- current epoch strictly less than expiry;
- exact digest, signature, and public-key lengths; and
- raw Ed25519 verification under the selected active issuer key.

The signed domain, network-domain bytes, Registry ID, record ID, issuer epoch,
sequence, not-before, and expiry prevent reuse across those values **only when
consumers pin them correctly**. A consumer's maximum-age limit is deliberately
not signed: it is stricter local policy applied to the signed not-before value.
Sequence prevents replay of an already accepted state; it does not prove that a
newly signed state reflects newly collected facts.
A compromised issuer can sign a false statement at the next sequence.

Cross-network separation is conditional. `network_domain` is arbitrary
administrator-provided bytes, not a value the Move VM derives from chain state.
Deployments must choose a canonical, unique network value, pin it in consumer
policy, use separate keys per network, and have the relayer independently
verify the RPC chain ID. Calling the bytes `sui:testnet` does not itself prove
that code is executing on Testnet.

## Freshness and the Sui Clock

V1 uses `tx_context::epoch`. Sui documents epoch timestamps as changing roughly
once every 24 hours, so this remains a coarse lifetime boundary:

- an attestation with a one-epoch TTL can last almost a day or expire soon after
  submission, depending on the epoch boundary;
- the protocol permits a Registry maximum up to 365 epochs; production
  deployments should configure a much shorter reviewed value;
- `AgentRecord` stores the signed `not_before_epoch`, and `VerifiedAgent`
  carries it into the consumer check; and
- `verify_bound` and `assert_verified_for` compare
  `current_epoch - not_before_epoch` with the consumer-owned
  `max_attestation_age_epochs` after first rejecting a future not-before.

The age comparison is inclusive. A maximum of `0` accepts only the current
epoch; a maximum of `5` accepts age five and rejects age six. This lets a
consumer be stricter than the Registry TTL, but different positions within one
epoch remain indistinguishable. It also makes the new arguments a deliberate
API change: every consumer must persist the age policy and pass the current
`TxContext`, including when re-checking a proof received inside a programmable
transaction block.

Applications that need minute- or second-level freshness should design a new
protocol version that takes the immutable Sui `Clock`, signs and stores an
observation timestamp, and carries it into the proof. The Clock updates at
checkpoint cadence and requires consensus. V1 must not reinterpret its epoch
fields as milliseconds.

## Public data and commitment privacy

Everything on Sui is public. Replacing plaintext with 32-byte hashes reduces
direct disclosure but does not provide confidentiality:

- low-entropy hostnames, policy labels, or small input sets can be
  dictionary-guessed;
- stable commitments and AgentRecord IDs are linkable across time;
- relayer/admin recipients, issuer keys, pause/revoke changes, transaction
  timing, sequence, not-before, and expiry remain observable;
- hashing a secret often exposes an offline verification oracle.

Use high-entropy deployment identifiers or an appropriate random salt, define a
domain-separated canonical commitment, and keep plaintext/salts according to a
reviewed disclosure and recovery policy. Never put prompts, outputs, customer
data, real hostnames, wallet identities, or secrets on chain, and never describe
an unsalted hash as anonymous or encrypted.

## Single-issuer and capability trust

There is exactly one active issuer. Staged keys are inactive until an
`AdminCap` holder rotates to one, which increments the issuer epoch. This keeps
verification simple and bounded, but creates a clear trust concentration:

- issuer compromise permits arbitrary valid-looking commitments until the
  Registry is paused; permanent invalidation requires rotation or per-record
  revocation;
- `AdminCap` compromise permits a new attacker-controlled issuer;
- pause is a temporary gate only: while paused, submission and verification
  fail, but unpausing does not change the issuer epoch or record status, so an
  otherwise unexpired, sufficiently fresh old record can verify again;
- rotation increments the issuer epoch and invalidates all existing records
  until they are re-attested; revocation permanently blocks the selected
  record;
- after an issuer compromise, rotate while paused before unpausing (or revoke
  affected records); and
- the contract does not implement M-of-N issuer approval.

Use a dedicated network-specific key in hardware or threshold custody, short
validity, alerting, rehearsed pause/rotation, and a separately governed
`AdminCap`. If the risk requires M-of-N authorization, implement and audit it
as a new protocol rather than describing an off-chain single key as threshold
security.

## Permissionless relay and shared-object contention

Anyone may call `submit_attestation`. A caller cannot alter signed fields
without invalidating the signature and pays transaction gas, but every attempt
requests mutable access to the shared AgentRecord. Invalid or replayed traffic
can therefore create consensus work and contention without changing state.
A hot record is also a serialization point for legitimate concurrent relays.

Permissionless relay improves failover but is not free denial-of-service
protection. Before production:

- load-test invalid-signature, replay, and same-sequence floods against a
  representative network;
- monitor transaction failure rate, latency, shared-object contention, relayer
  budget, and stale records;
- make issuer sequence allocation single-writer or otherwise prevent two
  different payloads being signed for the same next sequence; and
- decide explicitly whether to retain permissionless submission or add a
  relayer capability/allowlist in a new protocol version.

An allowlist reduces public contention but concentrates availability and
requires its own rotation and emergency policy.

## Upgrade and package identity

The audited package version is part of the trust decision. Sui upgrades publish
a new package object; old package versions remain callable. The V1 Registry has
a fixed protocol version and no migration entry point.

Choose one posture before publication:

### Immutable after audit

After tests, independent review, and controlled testnet validation, call
`sui::package::make_immutable` on the `UpgradeCap`. This gives consumers the
strongest code-stability claim. It is irreversible. A future fix needs a new
package, explicit Registry/record migration, and consumer allowlist migration;
old package calls remain possible until consumers retire them.

### Governed upgrades

If upgrades are required, keep `UpgradeCap` separate from operational
capabilities behind a high-threshold multisig or timelock, document and test the
upgrade runbook against representative state, and require explicit consumer
migration to the exact reviewed package version. A package name or original ID
alone is not enough evidence of reviewed code.

Do not promise both immutability and an in-place future upgrade. Record the
chosen package ID, version, Registry/record IDs, capability owners, transaction
digest, network domain, and consumer allowlists as deployment evidence in the
consumer fork, not as generic identities in this template.

## Reproducibility and current test evidence

`Move.toml` pins the Sui framework source to
`a9a6825eaf6273cc819ee3bcf65fd4909f7624a9`, which is the official
`mainnet-v1.66.2` tag. The source framework itself also uses the
`2024.beta` edition. This exact revision pin is useful, but it does not pin the
compiler or complete dependency graph.

The Sui package manager generates `Move.lock` and recommends committing it.
This package currently has no `Move.lock`, and the Tentaflake contributor
shell currently has no Sui CLI/compiler. A live Git dependency may therefore
need network/cache state and cannot be called an offline reproducible build.

The current evidence is:

| Area | Repository evidence | Status |
|---|---|---|
| BCS bytes and raw Ed25519 fixture | JSON fixture plus pure-standard-library Python verifier and one-byte mutation rejection | Executable without Sui; run it explicitly |
| Move BCS/Ed25519 vector | `tests/agent_attestation_tests.move` derives the payload from fixture fields through the production `SigningPayload` serializer, requires exact equality with the fixed 274 bytes, verifies the signature, and rejects a mutation | Test source present; not currently compiled |
| Registry configuration and AdminCap boundaries | Move tests for valid bounds, key/digest lengths, pause, duplicate/missing issuers, and cross-Registry capability misuse | Test source present; not currently compiled |
| Consumer maximum age | A test-only non-storable proof constructor drives inclusive-boundary acceptance plus stale and future-not-before rejection through `assert_verified_for` | Test source present; not currently compiled |
| Consumer authorization separation | `tests/consumer_capability_example.move` stores maximum age with pinned proof values, passes `TxContext`, and separately tests action-capability binding | Test-only source present; full submitted-record handoff remains untested |
| Privileged monitoring events | Test-only helpers in the defining module assert exact Registry/record/AdminCap IDs, recipient, key ID, issuer epoch, pause state, and public-key payloads for add/rotate/pause/AdminCap-transfer/revoke events | Test source present; `AttestationSubmitted` payload and positive submission remain untested; not currently compiled or localnet-indexed |
| Valid `submit_attestation` and `verify_bound` scenario | Requires deterministic object-bound signing data inside `test_scenario` | Missing |
| Signed replay/skip, expiry/backdate, post-rotation invalidation, post-revocation verification, attacker Registry, and contention scenarios | Deterministic signed shared-object tests and localnet load tests | Missing |
| Host collector, issuer, relayer, chain-ID/finality, budget, and secret isolation | No host integration exists in this source-only reference | Missing |

Run the independent fixture now:

~~~bash
python3 scripts/verify-sui-attestation-vector.py
~~~

Before treating Move behavior as verified:

1. package a reviewed Sui CLI/compiler matching `mainnet-v1.66.2`;
2. generate `Move.lock` with that package manager, commit it, and never hand-edit
   it;
3. materialize the source and dependencies through a pinned offline build
   closure rather than a live CI Git fetch;
4. run `sui move test` and record the exact CLI/compiler version;
5. add a deterministic `test_scenario` vector that signs the generated
   Registry and AgentRecord IDs and exercises successful submit/verify;
6. add exact-abort submitted-record cases for every signed field, replay/skip,
   future/backdated/expired windows, pause, revocation, rotation, wrong Registry
   and record, wrong consumer commitments, and attacker-created Registry; keep
   the focused consumer-age boundary cases as a separate policy layer;
7. publish to an isolated localnet, dry-run every critical transaction, measure
   gas and contention, and verify events/object state;
8. complete an independent Move security review and controlled testnet rollout.

Only after a real host collector/issuer/relayer exists do Nix evaluation and VM
tests become meaningful for disabled-by-default behavior, absence of agent chain
authority, wrong-chain/finality failures, relayer budgets, and secret-free
logs/store. A VM assertion against this source-only directory would not verify
the on-chain protocol.

## Deployment gate

Before any testnet or mainnet use, record evidence for every item:

- exact package version, dependency IDs, `Move.lock`, compiler, build command,
  and source revision;
- full Move positive/negative suite plus the independent signing vector;
- canonical schemas and digest algorithms for all four commitments;
- consumer-owned exact package, network-domain, Registry, AgentRecord,
  sequence/freshness, and commitment policy;
- separate actor/action authorization in every consuming entry point;
- AdminCap and UpgradeCap ownership, threshold, recovery, rotation, and chosen
  immutable-or-upgradeable posture;
- issuer custody, separate per-network keys, pause/rotation drill, and conflict
  prevention for sequence allocation;
- exact RPC chain-ID/finality checks, relayer transaction allowlist and wallet
  budget;
- public-data privacy review, salt/identifier handling, retention, and
  linkability acceptance;
- gas, shared-object contention, RPC failure, monitoring, and incident-response
  tests; and
- independent audit findings and explicit residual-risk acceptance.

Do not promote a warning, source review, Python-only check, or unavailable Sui
compiler into a passing Move build claim.

## Official primary sources

- [Sui v1.66.2: on-chain signature verification](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/docs/content/guides/developer/cryptography/signing.mdx)
- [Sui v1.66.2: BCS and object ID representation](https://github.com/MystenLabs/sui/tree/mainnet-v1.66.2/crates/sui-framework/docs/sui)
- [Sui v1.66.2: on-chain time and Clock](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/docs/content/guides/developer/on-chain-primitives/access-time.mdx)
- [Sui v1.66.2: TxContext epoch API](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/crates/sui-framework/packages/sui-framework/sources/tx_context.move)
- [Sui v1.66.2: event API](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/crates/sui-framework/packages/sui-framework/sources/event.move)
- [Sui v1.66.2: Move package management, Move.lock, and environments](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/docs/content/guides/developer/packages/move-package-management.mdx)
- [Sui v1.66.2: package versioning and old callable versions](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/docs/content/guides/developer/objects/versioning.mdx)
- [Sui v1.66.2: test_scenario API](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/crates/sui-framework/packages/sui-framework/sources/test/test_scenario.move)
- [Sui security best practices](https://docs.sui.io/develop/security/best-practices)
- [Sui production-readiness checklist](https://docs.sui.io/develop/production-readiness)
- [Sui shared objects](https://docs.sui.io/develop/objects/object-ownership/shared)
