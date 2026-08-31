# Sui agent attestation reference

This package is an experimental, optional integration for proving that an
authorized issuer observed a fresh Tentaflake agent-policy evidence bundle.
It is not imported by the NixOS module set, does not start a service, does not
configure an RPC endpoint, and does not publish a package or transaction.

A balanced agent must never receive a Sui RPC allowance, wallet/gas key,
issuer key, `AdminCap`, or arbitrary transaction signer. The intended flow is
host-side:

1. A fixed collector reads only declared, root-owned evidence.
2. A dedicated issuer signs the canonical BCS payload with an Ed25519 key held
   outside the agent, ideally through a hardware-backed or runtime credential
   channel.
3. A separate, budgeted relayer submits only the fixed
   `submit_attestation` programmable transaction.
4. Other Move packages call `verify_bound` with their own pinned Registry and
   AgentRecord IDs plus required commitments, or immediately call
   `assert_verified_for` on a handed-off proof. Accepting
   `VerifiedAgent` alone is unsafe and unsupported.

Do not place prompts, model responses, customer data, real host names, wallet
identities, or secrets on chain. The contract records opaque 32-byte
commitments only.

The contract allows exactly one active issuer key at a time. Additional keys are
staged inactive and take effect only through an epoch-changing rotation. It
uses raw Ed25519 over the exact BCS(SigningPayload) bytes shown in the guide;
it does not accept a Sui wallet personal-message or intent-framed signature.

Attestations use exact next sequences, exclusive expiry, and a deployment-set
backdating ceiling (at most seven epochs in this reference). They prove a
current issuer statement, not that the transaction sender is the agent. Bind
any actor or relayer authorization in the consuming Move package.

## Package pin and local verification

`Move.toml` pins Sui framework source to commit
`a9a6825eaf6273cc819ee3bcf65fd4909f7624a9`
(the upstream `mainnet-v1.66.2` tag at the time this reference was added).
Use a reviewed Sui CLI matching that framework before testing:

```bash
cd integrations/sui-agent-attestation
sui move test
```

The Tentaflake contributor shell does not currently ship a Sui compiler, so
this package is intentionally not part of `nix flake check` yet. V1 has no
migration entry point and should use an immutable UpgradeCap after audit; a
future version needs an explicit registry and consumer allowlist migration.
Do not treat source review as a substitute for Move compilation, cross-language
BCS test vectors, a security audit, or a controlled testnet deployment.

See [the Sui attestation guide](../../docs/17-sui-agent-attestation.md) for
the object model, canonical payload, security requirements, and rollout gate.
