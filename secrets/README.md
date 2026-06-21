# Secrets Directory

This directory holds **encrypted** `.age` files for [agenix](https://github.com/ryantm/agenix).

## What goes here

- `*.env.age` — encrypted agent environment files (API keys, tokens)
- `*.age` — any other encrypted configuration files

## What does NOT go here

- `*.env` — plaintext env files (gitignored, never committed)
- `*.key` — private keys (never committed)
- Any unencrypted credentials

## Quick Reference

```bash
# Create an encrypted secret for an agent
echo "OPENROUTER_API_KEY=sk-or-..." | agenix -e secrets/hermes-MYAGENT.env.age --stdin

# Edit an existing secret
agenix -e secrets/hermes-MYAGENT.env.age

# Wire it in my-agents.nix
mkHermesAgent {
  name       = "MYAGENT";
  agenixFile = "/run/agenix/hermes-MYAGENT-env";
}

# Verify after rebuild (check permissions, not contents!)
ls -l /run/agenix/
stat -c '%U %G %a %n' /run/agenix/hermes-MYAGENT-env
```

## Detailed Guide

See [`docs/04-agenix-secrets.md`](../docs/04-agenix-secrets.md) for the full setup guide.

See [`secrets.nix.example`](../secrets.nix.example) for the NixOS module template.
