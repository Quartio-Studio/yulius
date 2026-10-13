# 0037. Dead code is cleaned with each language's established tool; Yulius's gate uses cargo-shear

## Status

Accepted. Specified in
[SPEC-054](../../.ai/specs/SPEC-054-https-ktfish-github-io-analyse-what-tool.md).

## Context

People ask agents to "clean up" projects. Without guidance an agent greps by hand, misses
unused dependencies and removes code that is used dynamically. The ML in PL workshop that shaped
`infra-bootstrap`'s conventions audit recommends skylos and vulture (Python) "and other" tools.
Yulius onboards projects in any language, and its own gate checked unused Rust dependencies with
cargo-machete, which misses unused `[workspace.dependencies]` entries (`toml` was one).

## Decision

- A bundled `yulius` skill, `dead-code-cleanup`, is the procedure for every cleanup request:
  prefer the project's configured check, else the established tool per language from its table
  (searching for a better one when the stack is missing or a tool looks stale), verify every
  candidate, remove in small commits, keep the gate green, and record false positives in the
  tool's allowlist with a reason.
- Onboarding (the preparation prompt and `infra-bootstrap`'s gate step) adds a cheap,
  low-noise dead-code or unused-dependency check to a project's gate.
- Yulius's own `scripts/dev deps` runs `cargo shear` instead of `cargo machete`.

## Alternatives

- Instructions only in AGENTS.md: reaches this repository, not the projects Yulius onboards.
- One skill per language: the procedure is the same; only the tool table differs.
- Keeping cargo-machete next to cargo-shear: two tools for one check, and shear found everything
  machete did plus the workspace dependency.
- vulture and skylos in Yulius's gate: its Python scripts are clean and the gate installs no
  Python packages; the skill runs them on demand.

## Consequences

- The bundled set grows by one skill (SPEC-001 SKL-007).
- The first CI run after the change compiles cargo-shear once (about two minutes); the tool cache
  keeps it afterwards.
- A dependency used only through a macro that cargo-shear cannot see needs an
  `[package.metadata.cargo-shear] ignored` entry with a comment.
