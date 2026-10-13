---
yulius-task: 25304c92-ee5c-4317-beae-4f7bbd008b71
status: done
---

# SPEC-054: Dead-code tools in onboarding, in the bundled skills and in this repository

Status: implemented
Date: 2026-10-13
Owner: Yulius task #38
Scope authorization: user request (F 2026-10-13, task #38): "https://ktfish.github.io/ Analyse
what tools for dead code are mentioned here and make them part of the onboarding. Also, make them
part of the current repo as well. Agent should be use them when asked for cleaning etc. Identify
popular tools for different language but the agent also can search for their own".

## Goal

When a person asks an agent to clean up a project (dead code, unused imports, unused
dependencies, stale files), the agent uses the established tool for each language the project
has, verifies every candidate before removing it, and keeps the gate green. Onboarding sets such
a check up in a project's gate, and Yulius's own repository uses one.

## Context

- The ML in PL workshop site (ktfish.github.io) names dead-code tools once, on its "Further
  resources" page: "Tools for cleaning so called dead code: skylos, vulture and other", linking
  `duriantaco/skylos` and `jendrikseipp/vulture`. Both are Python tools. Its "instrumentation
  audit" page lists dead code, unused experiments and stale notebooks as anti-patterns to remove
  before release; `infra-bootstrap` step 6 already adopted that audit (SPEC-005 APM-005).
- Yulius ships bundled skills to every sandbox (`catalog/skills/<pack>`, embedded by
  `skills/pack.rs`, SPEC-001 SKL-007) and onboards an imported project with a preparation prompt
  (`onboarding.rs`, DSC-010) that sends missing foundations to `infra-bootstrap`. The Chore
  workflow already names "dead-code removal" but no tool.
- Yulius's own gate: rustc's `dead_code`/`unused_*` warnings fail `clippy -D warnings`,
  `cargo-machete` checks unused dependencies, and `check_repository.py` fails on unused web UI
  imports. Measured on this branch: vulture (80 % confidence) and skylos find nothing in
  `scripts/`; `cargo shear` finds the workspace dependency `toml`, declared in
  `[workspace.dependencies]` but used by no member, which cargo-machete misses.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DCT-001 | A bundled `yulius` skill `dead-code-cleanup` is used when an agent is asked to clean up, remove dead or unused code, imports, dependencies or files. It names the established tools per language (Python: vulture and skylos from the workshop, ruff, deptry; Rust; JavaScript/TypeScript; Go; Java/Kotlin; C#; PHP; Ruby; Swift; Dart; C/C++; Elixir), tells the agent to search for the current best tool when the stack is not listed or a tool looks stale, prefers the project's own configured check, and requires verifying each candidate (dynamic use, public API, entry points, generated code) before removing it in small commits with the gate green. | `embedded_packs_match_the_catalog`, `skills_mcp.rs` run set |
| DCT-002 | Onboarding names the skill: the preparation prompt asks for a dead-code and unused-dependency check in the gate, and `infra-bootstrap`'s gate step lists it. The Chore workflow sends dead-code removal to the skill. | `onboarding.rs` prompt test, `default_workflows_follow_the_discipline` |
| DCT-003 | Yulius's own gate (`scripts/dev deps`) checks unused dependencies with `cargo-shear` instead of `cargo-machete`, and the unused workspace dependency `toml` is removed. AGENTS.md and the development guide say which tools clean this repository. | `scripts/dev ci` |
| DCT-004 | SPEC-001 SKL-007, the skill pack READMEs, `docs/engineering/development.md`, an ADR and `docs/changes.md` describe it. | review |

## Plan

- [x] Spec SPEC-054 and ADR for dead-code tooling
- [x] Bundled dead-code-cleanup skill with per-language tools, shipped in the yulius pack
- [x] Onboarding, infra-bootstrap and chore workflow route cleanup to the skill
- [x] This repo: cargo-shear in the deps gate, unused toml dependency removed, AGENTS.md routes cleanup
- [x] Every gate green, independent review, changes log updated
- [x] Merged into dev on green CI

## Acceptance criteria

- AC-DCT-1: a sandbox's run set of skills contains `dead-code-cleanup` next to the other
  default-enabled bundled skills, installed into the harness homes.
- AC-DCT-2: the onboarding preparation prompt contains `dead-code-cleanup` and stays under the
  prompt size limit.
- AC-DCT-3: `scripts/dev deps` runs `cargo shear` and fails on an unused dependency; it passes
  on this branch after `toml` is removed.
- `scripts/dev ci` passes.

## Test plan

- Unit: `embedded_packs_match_the_catalog` fails until the new catalog file is embedded; the
  onboarding prompt test asserts `dead-code-cleanup` (red before the prompt names it).
- Integration: `skills_mcp.rs` asserts the run set includes `dead-code-cleanup` (red before the
  pack lists it).
- Gate: `scripts/dev deps` with `cargo shear` fails on `main`'s `toml` entry, passes after its
  removal.

## Decisions

- One universal skill rather than per-language skills: the workflow (detect, verify, remove,
  gate) is the same everywhere; the table of tools is a reference the agent extends by searching.
- The skill is enabled by default like the other `yulius` skills, so a cleanup request in any
  sandbox finds it.
- `cargo-shear` replaces `cargo-machete` in Yulius's gate: it also checks workspace
  dependencies and caught one machete missed. See
  [ADR-0037](../../docs/decisions/0037-dead-code-tools.md).
- vulture and skylos are not added to Yulius's own gate: its eight Python scripts are clean, and
  the gate installs no Python packages today; the skill runs them on demand (`uvx`/`pipx run`).

## Evidence

- Source analysis: the site's 20 pages were fetched; only "Further resources" names dead-code
  tools ("skylos, vulture and other"); the instrumentation-audit page lists dead code as an
  anti-pattern. Both tools were installed (vulture 2.16, skylos 4.48.0) and run on `scripts/`:
  nothing at 80 % confidence, and skylos reports no candidates.
- Red first: `skills_mcp::skill_catalog_sources_toggles_and_run_set` failed (run set without
  `dead-code-cleanup`) before the pack listed the skill and passes after;
  `onboarding::tests::prompt_names_problems_profile_and_rules` failed at the new
  `dead-code-cleanup` assertion before the prompt named it and passes after.
- Gate: with `cargo shear --locked --deny-warnings` in `scripts/dev deps`, the gate failed on
  `unused workspace dependency toml` and passes after its removal; an `hex.workspace = true`
  injected into `yulius-contracts` (reverted) made cargo-shear fail with `unused dependency hex`.
- `scripts/dev ci` on 801456e: exit 0; nextest 1223 passed, 16 skipped; checker tests 37 OK;
  repository contracts, fmt, clippy, doctests, cargo-deny, cargo-shear and rustdoc green. The
  later review fixes (6483068) change Markdown only; `skills`, `onboarding`, `workflow` and
  `skills_mcp` tests and `check_repository.py` pass on it.
- Review round 1 (fresh read-only session): APPROVE at 801456e with 4 minors and 6 nits on the
  tool table and the development guide (`tsc --noEmit`, .NET `EnforceCodeStyleInBuild`, C/C++
  section flags, section placement, knip `ignoreDependencies`, wording); all fixed. Round 2:
  APPROVE at 6483068.
- Not covered: a real onboarding run of an imported project with the new prompt, and the
  per-language commands for languages other than Python and Rust, which were reviewed from
  the independent reviewer's knowledge (vulture, skylos and cargo-shear flags confirmed by
  running them) rather than run here.
