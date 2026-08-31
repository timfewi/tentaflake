# Sui agent attestation

The Sui agent-attestation package is an experimental, optional reference for
verifying fresh Tentaflake evidence in Move. Its source lives in
integrations/sui-agent-attestation. It is not a NixOS module, adds no default
networking or service, holds no wallet, and does not publish anything
automatically.

The chain can prove only that an authorized issuer made a fresh statement about
specific commitments. It cannot prove that an agent, model, image, dependency,
or retrieved content is safe, correct, or harmless.

## Trust boundary

A balanced agent must not receive:

- Sui RPC access, a gas key, a wallet, an issuer key, or an AdminCap;
- an arbitrary transaction signer or arbitrary transaction payload relay;
- a Docker or Podman socket; or
- control of the evidence file that a host-side collector signs.

The intended implementation has three separate host-owned roles:

1. A fixed collector reads only declared, root-owned facts and produces a
   minimal evidence bundle.
2. A dedicated Ed25519 issuer signs the canonical payload. Store its key as a
   runtime credential or in hardware-backed signing infrastructure, never in
   Nix, Git, a prompt, a derivation, command-line arguments, or an agent
   workspace.
3. A separate relayer has a small gas budget and may submit only the fixed
   submit_attestation transaction to an exact HTTPS RPC endpoint after
   chain-ID and finality checks.

The AdminCap should be transferred to an operator multisig or timelock, not to
the host that collects or relays attestations.

## Object model

The package uses a shared Registry object and one shared AgentRecord object per
logical agent.

| Object | Ownership and purpose |
|---|---|
| Registry | Shared; one active issuer plus bounded staged keys, fixed protocol version, network domain, pause switch, maximum lifetime, and maximum backdating |
| AgentRecord | Shared; opaque agent-handle commitment, status, exact monotonic sequence, issuer epoch, exclusive expiry, and four 32-byte commitments |
| AdminCap | Address-owned; no store ability, so only the defining module can transfer it |
| VerifiedAgent | Has drop but not store; a same-transaction handoff proof, never a standalone authorization token |

Sharing an object only makes it addressable. Every mutation checks either the
registry-bound AdminCap or the one active issuer signature. Extra issuer keys
are staged inactive; rotating to one changes the issuer epoch and invalidates
previous evidence.

A receiving package must call `verify_bound` using Registry and AgentRecord
IDs that it pins in its own configuration, along with the exact policy, image,
Golden-eval, and evidence commitments it requires. If a proof crosses a
programmable transaction block boundary, it must immediately call
`assert_verified_for` with those same trusted values. A package that accepts
only the universal `VerifiedAgent` type can be fooled by an attacker-created
registry and is insecure. This attestation also does not establish the
transaction sender's identity; bind an actor capability or relayer policy in
the consumer when that matters.

The record stores only the latest valid state. It has no unbounded history,
dynamic fields, or plaintext deployment data. Revocation changes status rather
than deleting the object, so consumers can distinguish revoked from missing.

## Canonical signed payload

The issuer signs BCS bytes for this ordered payload:

~~~text
SigningPayload {
  domain: "TFA-SUI-ATTEST-v1",
  network_domain,
  registry_id,
  agent_record_id,
  protocol_version,
  issuer_key_id,
  issuer_set_epoch,
  sequence,
  not_before_epoch,
  expires_epoch,
  policy_digest,
  image_digest,
  golden_eval_report_digest,
  evidence_digest
}
~~~

All four digest fields are exactly 32 bytes. The signer mode is raw Ed25519
over exactly BCS(SigningPayload); it is not a Sui wallet personal-message,
intent-framed, JSON, or implementation-defined prehash signature. Cross-language
test vectors must pin this byte layout and the digest algorithms before use.

The on-chain code reconstructs the payload from the Registry and AgentRecord
object IDs, checks the current issuer-set epoch, an exact next sequence,
exclusive expiry, a maximum lifetime, and a bounded backdating window before
verifying the signature and replacing the record's latest commitments.

This prevents replay across domains, networks, registries, records, issuer
rotations, sequences, and expiry windows. A pause, revocation, or issuer
rotation makes verification fail closed until a fresh attestation is submitted.

Golden-eval results belong on chain only as a commitment to a versioned report:
suite version, test revision, build and runner evidence, and outcome. Never
store test prompts, outputs, customer data, or readable agent names.

## Package and test gate

Move.toml pins the Sui framework source to commit
a9a6825eaf6273cc819ee3bcf65fd4909f7624a9. The source fixes
CURRENT_PROTOCOL_VERSION to 1 rather than accepting a caller-selected version,
and every security-sensitive V1 path checks it. V1 has no migration entry
point: after audit, make its UpgradeCap immutable. A future upgrade needs an
AdminCap-authorized migration that changes the registry version so old code
rejects it, plus a consumer allowlist migration; old V1 objects remain callable
until consumers retire them. Use a reviewed matching Sui CLI and run:

~~~bash
cd integrations/sui-agent-attestation
sui move test
~~~

The current Tentaflake contributor shell does not ship the Sui compiler, so
this package is not yet in nix flake check. Before any testnet or mainnet
deployment, add and pass:

- Move positive and negative tests for invalid keys, signatures, domains,
  registry/record bindings, replay, expiry, pause, revocation, issuer
  rotation, capability misuse, and bounded fields;
- fixed cross-language Rust-to-Move BCS and raw-Ed25519 vectors;
- a reproducible, pinned Sui CLI/compiler derivation with no live Git fetch in
  CI or release verification;
- host tests for missing credentials, wrong chain ID, RPC and finality errors,
  relayer budget exhaustion, and absence of secrets from logs and the Nix
  store;
- a VM test proving that disabled means no service or agent egress, and that
  enabled mode still grants agents no direct chain authority; and
- an independent Move security review before a package becomes immutable.

Use an immutable package after audit where possible. A consumer must allowlist
the exact audited package, Registry, AgentRecord, and network-domain values.
If an upgrade is unavoidable, custody the UpgradeCap in a multisig or timelock,
require an explicit consumer migration, and retire old allowlist entries; old
package IDs remain callable.
