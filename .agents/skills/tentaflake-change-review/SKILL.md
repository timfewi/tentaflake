---
name: tentaflake-change-review
description: Lightweight change-review discipline for the tentaflake repo — require a linked reason (issue/requirement), a verification step, and doc sync before any change is called done. Use when planning, implementing, or reviewing a change to the template.
version: 1.0.0
---

# Tentaflake — Change Review

## Overview

A deliberately small verification discipline for changes to the tentaflake
template. It borrows one idea from heavyweight AI-engineering frameworks —
**traceability: every change ties back to a reason and forward to a check** —
but strips the ceremony (no ISO matrices, no multi-cycle lifecycle, no
red-team gates). It is sized for a ~30-file NixOS template maintained by
Conventional Commits + PRs.

The goal: stop "orphaned" changes (code with no stated reason), silent
assumptions, and doc drift — without slowing ordinary work.

## When to use

- Planning any non-trivial change (new module, option, lib arg, script, CI step).
- Implementing a bug fix or feature.
- Reviewing a diff before opening or merging a PR.

Skip it for pure formatting, typo fixes, or comment-only edits.

## The three gates

Every change must clear three gates before it is "done". State them explicitly
in your plan and in the PR description.

### 1. REASON — why does this change exist?

Link the change to one of:

- an existing GitHub issue (`gh issue view <n>`),
- a bug reproduced with a concrete command/output, or
- a one-sentence requirement written *before* the code.

If you cannot name the reason, do not write the code. A change whose only
justification is "seemed nice" is an orphan — either write the requirement
first or drop it.

> Anti-pattern: adding a `tentaflake.*` option "just in case". Options are API
> surface; each one needs a stated need and a default that keeps existing
> configs working.

### 2. VERIFY — how do we know it works?

Name the check *before* implementing, and actually run it. Pick the tightest
applicable command from the repo's real toolchain:

| Change touches | Verification |
|---|---|
| Any `.nix` (module/lib/config) | `nix flake check` |
| Host config / new option | `nix build .#nixosConfigurations.agent-host.config.system.build.toplevel --no-link` |
| Formatting | `nix fmt` (CI runs `nix fmt -- --ci`) |
| Go (`pkgs/tentaflake-auditd`) | `go build ./... && go vet ./... && go test ./...`, then `golangci-lint run` |
| Shell scripts | `shellcheck installer/*.sh scripts/*.sh` |
| Login banner | `./scripts/banner-test.sh` |
| ISO changes | `nix build .#installer-iso` / `nix build .#live-agent-iso` |

Prefer a fresh check over self-grading: don't just assert "it builds" — paste
the command you ran. If a behavior can't be exercised by an existing check,
add or extend a check (e.g. a Go test) as part of the same change.

> Anti-pattern: "passed all tests" when the change added no test and no
> existing test covers the new path.

### 3. SYNC — what else must move with it?

Per `AGENTS.md`, a change that alters behavior, options, or usage is not done
until its docs move in the same change. Check each surface:

- `README.md` and `docs/` — user-facing behavior/options.
- `AGENTS.md` / `CLAUDE.md` — agent instructions.
- `.agents/skills/tentaflake-repo-guidance/SKILL.md` — option tables, lib args,
  module reference (this is the map agents rely on; stale entries mislead).
- Relevant `.agents/skills/` — if you touched a subsystem a skill documents.
- `my-agents.nix.example` / `*.env.example` — if you changed agent args or env.
- `CHANGELOG` entry if the repo keeps one.

## Traceability (the through-line)

The three gates only pay off if the links between them are *recorded*, not just
thought about. Keep a single, lightweight chain per change:

```
REASON (issue/requirement)  →  ART (the commit/diff)  →  CHECK (verification)
```

You don't need a separate matrix file — git and GitHub already store the chain
if you populate them:

- **REASON → ART:** reference the issue in the commit body and PR
  (`Closes #NN`, or `Refs: <one-line requirement>` when there's no issue).
  GitHub then back-links the commit/PR to the issue automatically.
- **ART → CHECK:** name the verification command in the commit/PR body
  (`Verified: nix flake check`) and, where the behavior is testable, add the
  test in the *same* commit so the artifact and its check live together.
- **One change = one chain.** Keep unrelated reasons in separate commits/PRs so
  each link is unambiguous. A commit that closes three unrelated issues has no
  clean trace.

Recommended commit-body footer (Conventional Commits subject stays as-is):

```
feat(shell): add `tentaflake doctor` egress check

Closes #123
Verified: nix flake check; ./scripts/banner-test.sh
Docs: README.md, tentaflake-repo-guidance SKILL.md
```

To audit the chain later:

```bash
git log --oneline --grep 'Closes #123'   # REASON → ART
gh issue view 123                         # see linked PRs/commits
```

If you cannot fill in all three lines of the footer, one of the gates was
skipped — go back and close it before merging.

## Template guardrails (hard stops)

These override everything above — a change that violates one is rejected
regardless of REASON/VERIFY/SYNC:

- **No domain-specific content.** No real hostnames, company config, hardware
  configs, API keys, secrets, or deployment-specific SOUL.md/skills. That work
  belongs in a fork (`docs/05-fork-checklist.md`).
- **No secrets in the tree.** gitleaks runs in CI; don't commit `.env` files
  with real values.
- **Backward compatibility for options.** Renames use
  `mkRenamedOptionModule`; new options default to preserving current behavior.

## PR checklist mapping

When you open the PR, fill `.github/PULL_REQUEST_TEMPLATE.md` so the three
gates are visible:

- `## Description` → the **REASON** (what changed and why).
- Type-of-Change box → ticked to match the reason.
- Checklist items → the **VERIFY** commands you ran + **SYNC** surfaces you
  updated. Tick every applicable box; leave inapplicable ones unchecked
  (don't delete them).

## Quick self-audit before saying "done"

1. Can I name the issue/requirement this closes? (REASON)
2. Did I run a concrete check and see it pass? (VERIFY)
3. Did I grep for every doc/example that references what I changed? (SYNC)
4. Is the REASON→ART→CHECK chain recorded in the commit/PR footer (`Closes` / `Verified` / `Docs`), not just in my head? (TRACEABILITY)
5. Did I introduce any domain-specific or secret content? (guardrail — must be no)

If any answer is unsatisfying, the change is not finished.
