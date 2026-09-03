# Sui agent attestation reference

This package is an experimental, optional reference for checking an
issuer-authorized statement over opaque Tentaflake evidence commitments. More
precisely, the Move code checks that the one active Registry key produced a
valid raw Ed25519 signature over the exact BCS payload, that its object,
sequence, issuer-epoch, and signed epoch-window bindings are current, and that
the consumer-selected maximum-age policy holds.

That cryptographic result does **not** prove that the issuer observed the
claimed host state, that the commitments describe genuine evidence, or that an
agent is safe. It is an issuer statement, not remote attestation and not an
agent identity or authorization protocol.

The package is not imported by the NixOS module set, starts no service,
configures no RPC endpoint, and publishes no package or transaction.

## Three separate decisions

Do not collapse these three checks into one:

| Decision | What V1 supplies | What the consumer must still supply |
|---|---|---|
| Evidence binding | Signature, Registry and AgentRecord IDs, sequence, issuer epoch, signed epoch window, and four exact commitments | Canonical evidence schemas, digest algorithms, a maximum acceptable attestation age, and confidence in collector and issuer behavior |
| Logical identity | An immutable AgentRecord ID and opaque handle commitment selected by the Registry administrator | An allowlist that maps that exact record to the deployment identity the consumer trusts |
| Authorization | Nothing about the transaction sender or intended action | A consumer-owned policy object and actor/action capability, role, or other explicit authorization check |

A consumer must pin trusted Registry and AgentRecord IDs, minimum sequence,
maximum acceptable attestation age, and required commitments in its own
authenticated state. Forwarding transaction-supplied “expected” values is not
pinning. Both `verify_bound` and `assert_verified_for` take that consumer-owned
`max_attestation_age_epochs` and the current `TxContext`. If a `VerifiedAgent`
value crosses a programmable transaction block boundary, the receiving package
must immediately call `assert_verified_for` and then apply its separate
actor/action authorization. Accepting `VerifiedAgent` alone is unsafe.

## Intended host-side flow

A balanced agent must never receive a Sui RPC allowance, wallet/gas key, issuer
key, `AdminCap`, `UpgradeCap`, or arbitrary transaction signer. The intended
roles are separate and host-owned:

1. A fixed collector reads only declared, root-owned evidence and emits a
   versioned, canonical evidence bundle.
2. A dedicated issuer signs exactly BCS(SigningPayload) with a network-specific
   Ed25519 key outside the agent, preferably through an HSM or threshold
   service.
3. A distinct relayer with a small wallet budget submits only the fixed
   `submit_attestation` transaction to an exact endpoint after chain-ID and
   finality checks.
4. A consuming Move package checks its own trusted policy and authorization
   capability before acting.

The contract has exactly one active issuer. Compromise of that key permits
false statements. Pausing blocks submission and verification only while the
Registry remains paused; it does not invalidate old records. After compromise,
rotate the issuer while paused or revoke affected records before unpausing. The
chain cannot prove HSM use, role separation, or honest collection. Put
`AdminCap` behind a reviewed multisig or compatible governance path, keep the
issuer key separate, use short validity windows, and monitor both.

## Time, network, privacy, and availability

V1 measures validity in Sui epochs. Sui documents epoch time as changing roughly
once every 24 hours, so “fresh” is intentionally coarse. The signed
`not_before_epoch` is stored in `AgentRecord`, copied into `VerifiedAgent`, and
checked against the consumer's `max_attestation_age_epochs` using the current
`TxContext`. The comparison is inclusive: `0` accepts only an attestation whose
not-before value is the current epoch, while `5` accepts age five but rejects
age six. This consumer rule can be stricter than the Registry TTL, but it cannot
provide sub-epoch freshness. A protocol requiring minute-level freshness should
use the read-only Sui `Clock`, sign and store an observation timestamp, and
carry it into the proof. That is a versioned protocol change, not a deployment
toggle for V1.

`network_domain` is administrator-provided bytes. It separates signatures
across networks only when deployments choose a unique canonical value and
consumers pin it; the Move VM does not prove that it equals the active chain ID.
The relayer must still verify chain ID and finality off chain.

Do not put prompts, responses, customer data, real host names, wallet identities,
or secrets on chain. The contract stores opaque 32-byte values, but a hash is
not encryption: low-entropy names can be guessed, stable commitments are
linkable, and relayer timing is public. Hash only a documented,
domain-separated canonical schema using high-entropy identifiers or an
appropriate salt. Never hash a secret directly and call the result private.

`submit_attestation` is permissionless and mutates a shared AgentRecord.
Invalid callers pay their own gas, but a flood can still contend on that shared
object. A deployment must load-test this tradeoff and monitor failures. If
availability requires an allowlisted relayer capability, that is a deliberate
protocol change that reduces permissionless failover.

Typed events cover `AdminCap` transfer, issuer addition and rotation, pause
changes, revocation, registration, and attestation submission. The attestation
event includes both `not_before_epoch` and `expires_epoch`, but deliberately
omits the signature and commitment bytes; indexers must read the transaction
and resulting record when those details are required. Events are monitoring
signals, not an authorization boundary, and their public contents inherit the
same privacy/linkability caveats as object state.

## Package pin and verification

`Move.toml` pins Sui framework source to commit
`a9a6825eaf6273cc819ee3bcf65fd4909f7624a9`, the upstream
`mainnet-v1.66.2` tag. That is a useful source pin, but it is not a complete
reproducible toolchain. The package currently has no committed `Move.lock` and
Tentaflake does not pin or ship a matching Sui CLI/compiler.

The committed V1 vector contains deterministic **public test material only**;
it includes no private seed:

```bash
python3 scripts/verify-sui-attestation-vector.py
```

That standard-library verifier independently reconstructs the 274-byte
BCS(SigningPayload), verifies the raw Ed25519 signature, and requires a
one-byte payload mutation to fail. Move test sources derive the same bytes
through the production `SigningPayload` serializer before checking equality,
signature, and mutation; they also cover configuration/capability boundaries,
typed administrative events, inclusive consumer-age acceptance plus
stale/future rejection, and a separate consumer-capability pattern. They are not compiler evidence until run with the
pinned CLI:

```bash
cd integrations/sui-agent-attestation
sui move test
```

Before calling the package build reproducible, use the reviewed CLI matching
the framework, generate `Move.lock` through the Sui package manager, commit
that generated lockfile, and make CI consume only the pinned/offline dependency
closure. Never hand-edit `Move.lock`.

Before publication, choose one explicit upgrade posture:

- after independent audit, destroy the `UpgradeCap` with
  `sui::package::make_immutable`; later fixes then require a new package and
  explicit Registry/consumer migration; or
- retain a governed `UpgradeCap` behind a high-threshold multisig or timelock,
  test upgrades against representative state, and require consumers to migrate
  their exact package allowlist. Old package versions remain callable.

Source review and the Python vector do not replace Move compilation, negative
scenario tests, an independent security audit, gas/contention testing, or a
controlled testnet deployment.

## Official Sui sources

- [Sui v1.66.2 on-chain signature verification](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/docs/content/guides/developer/cryptography/signing.mdx)
- [Sui v1.66.2 on-chain time and Clock](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/docs/content/guides/developer/on-chain-primitives/access-time.mdx)
- [Sui v1.66.2 TxContext epoch API](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/crates/sui-framework/packages/sui-framework/sources/tx_context.move)
- [Sui v1.66.2 event API](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/crates/sui-framework/packages/sui-framework/sources/event.move)
- [Sui v1.66.2 Move package management and Move.lock](https://github.com/MystenLabs/sui/blob/mainnet-v1.66.2/docs/content/guides/developer/packages/move-package-management.mdx)
- [Sui security best practices](https://docs.sui.io/develop/security/best-practices)
- [Sui production-readiness checklist](https://docs.sui.io/develop/production-readiness)

See [the Sui attestation guide](../../docs/17-sui-agent-attestation.md) for the
full claim boundary, payload, test matrix, and rollout gate.
