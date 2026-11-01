---
name: tentaflake-change-review
description: Review a concrete Tentaflake requirement, verification evidence and synchronized documentation.
version: 2.1.0
---

# Change review

1. Name the requirement and inspect the owning source before editing. Preserve
   unrelated work; read `AGENTS.md` for architecture/privacy/security contracts.
2. Select focused verification using [build guidance](../../../docs/17-builds.md).
   Start with `just fast`; locally, evaluation, builds and runtime acceptance are
   separate gates, while CI runs every affected gate and all must pass. Delegate
   workers skip checks until the integration owner runs them.
3. Review the complete diff for caller/schema/identity migration and state
   preservation. No silent security downgrade, implicit quota/budget reset or
   claim of vendor acceptance from a synthetic test.
4. Synchronize affected examples, guides and skills. Runtime facts belong only
   in `adapters/<id>.nix`; regenerate catalog projections after changes. `docs/`
   is product documentation only; plans, notes and private deployment facts go
   to the ignored `.scratch/`, never into Git.
5. Report actual commands/results and missing evidence. A build never authorizes
   activation. Fill every applicable PR-template section/checklist and use DCO
   sign-off with the human's configured identity when committing is authorized.

Keep docs compact and crosslink authoritative owners. The public repository
owns docs; website publication is separate and follows a pinned release. See
[CONTRIBUTING](../../../CONTRIBUTING.md).
