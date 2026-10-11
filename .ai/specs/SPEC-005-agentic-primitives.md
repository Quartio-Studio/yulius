---
yulius-task: 7b8158c8-af9b-4985-9d9e-c1a640027b4c
status: done
---

# SPEC-005: Agentic primitives are packaged and designed the APM way

Status: implemented
Date: 2026-10-11
Owner: Yulius task #3
Scope authorization: user request ("Scan this deeply https://ktfish.github.io/ and adjust our
methods to it. For sure use agent package manager but also other things from the tutorial. Bake
the approach into our approach")

## Goal

Yulius's own agent primitives (skills) are managed like code: one source, one manifest, a lockfile,
deployment to every harness, drift detection, and a written design standard (patterns,
anti-patterns, conventions audit) that every new primitive and every onboarded project follows.

## Context

The source is the ML in PL workshop "agentic coding methodology" (ktfish.github.io, based on
*Agentic SDLC* and microsoft/apm). Read in full (19 pages). What it teaches, and where we stand:

- **APM** (`apm.yml` manifest, `apm.lock.yaml`, `.apm/` sources, deployment to per-harness
  directories, `apm audit` drift check, pinned external packages). Today `.agents/skills` is
  hand-edited, with no manifest, no lock and no drift check; `.claude/skills` does not exist.
  Verified here: `apm-cli` 0.33.0 deploys `.apm/skills/*` to `.agents/skills/` and
  `.claude/skills/` for targets `claude` + `codex`, and `apm audit` reports "No drift detected".
- **Design patterns and anti-patterns for primitives** (lazy asset, persona preload with scoped
  tools, thread spawn with a JSON receipt, orchestrator facade, validation decorator, deterministic
  tool bridge, conditional dispatch; God module, eager bloat, toolless assertion, wrapping without
  blocking, unprompted drift). Our review and workflow skills already do some of this implicitly;
  none of it is written down or checked.
- **Instrumentation audit**: list conventions, classify where they live, rank by failure cost, map
  each to a primitive type (instruction, agent, skill, prompt, memory, spec, hook), write a starter
  set. Our `infra-bootstrap` skill lays gates and docs but never captures conventions as primitives.
- **Skill frontmatter rules**: description starts with "Use when", states negative cases, and
  changing it is a breaking change. Our checker only checks name/description shape.
- Not adopted: Genesis catalogue wrapper and `waza` benchmarks (evaluate later), `gh-aw`
  (GitHub-hosted agent workflows; conflicts with ADR-0015), the LaTeX exercise itself.
- Yulius already discovers `.agents/skills` and `.claude/skills` in a sandbox repository
  (`REPOSITORY_SKILL_DIRS`), so deployed output stays compatible with the product.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| APM-001 | `.apm/skills/<name>/` is the only place repository skills are edited; `apm.yml` declares the package and targets `claude` and `codex`; `apm.lock.yaml` is committed; `.agents/skills` and `.claude/skills` are generated deployments. | `check_repository.py`, `apm audit` |
| APM-002 | The repository check fails when a deployed skill differs from its source (hand-edit or missing deployment), without needing the `apm` binary. | `test_check_repository.py` drift cases |
| APM-003 | The repository check enforces the authoring rules: description contains a "Use when/for/on/before/after/this ..." sentence naming when to use it; a body over 200 lines must move detail to `references/` or `assets/`. | `test_check_repository.py` |
| APM-004 | `docs/engineering/primitives.md` is the design standard: primitive types and the convention-to-primitive map, the pattern and anti-pattern vocabulary, receipts, description-as-contract, pinning of external packages. A skill `yulius-primitives` applies it. | link check, skill check |
| APM-005 | `infra-bootstrap` gains a conventions audit step (list, classify, rank by failure cost, map to primitive, starter set) so each onboarded project gets its conventions codified. | catalog skill test |
| APM-006 | AGENTS.md, README, `docs/engineering/{development,workflow}.md`, architecture, product, changes and an ADR describe the change. | spec_check, link check |

## Plan

- [x] Spec and ADR 0016 record the APM decision
- [x] Repository skills live in `.apm/skills`, deployed by `apm install` with a committed lock
- [x] The repository check fails on skill drift and on weak skill descriptions or oversized bodies
- [x] A primitive design standard and the `yulius-primitives` skill are in place and linked
- [x] `infra-bootstrap` audits conventions; docs and changes are current; gates green

## Acceptance criteria

1. Editing a deployed `.agents/skills/*/SKILL.md` by hand makes `python3 scripts/check_repository.py` fail naming the file; `apm install` restores it.
2. `apm audit` in a clean checkout reports no drift.
3. A skill whose description has no "Use when/for ..." sentence fails the check.
4. A freshly onboarded project's `infra-bootstrap` run includes the conventions audit.

## Test plan

- Python unit tests (`scripts/test_check_repository.py`): drift detected for changed, missing and extra deployed files; clean tree passes; description and size rules fail closed.
- Catalog skill test for the new step if the existing catalog tests pin skill text.
- Manual: `apm install` then `apm audit` (needs `apm-cli`; CI cannot assume it, hence APM-002 is pure Python).

## Decisions

- Adopt APM with `.apm/skills` as source over keeping hand-edited `.agents/skills`: one manifest and lock, multi-harness deployment, drift detection. Rejected: symlinks (break on Windows and in the sandbox's copy semantics), a custom sync script (reinvents APM).
- Commit the deployed directories: Yulius sandboxes and plain `git clone` consumers read them without installing `apm`. The pure-Python drift check keeps the gate independent of the binary.
- Targets `claude` + `codex` only (what Yulius runs and what `.agents` serves); add more when someone needs them.
- Vendored `catalog/skills` stay as they are (embedded in the binary, ADR-0006); moving them to pinned external APM dependencies is a follow-up issue.
- The rule is "contains a Use sentence", not "starts with Use": existing skills lead with what they do. Deployments are compared as bytes; `.gitattributes` pins LF so checkouts match. Discovery already dedupes `.agents`/`.claude` copies by name (skills.rs precedence).
- Mapping only the patterns we use into the standard, not the full catalogue; link the source for the rest.

## Evidence

- `apm install` (apm-cli 0.33.0): 7 skills integrated into `.agents/skills/` and `.claude/skills/`; `apm audit`: "No drift detected" (run before the last skill was added; the lock was regenerated by `apm install` after).
- `python3 -m unittest discover -s scripts`: 36 tests OK (new: description rule, body size boundary 200/201, drift for changed, missing and extra files).
- `python3 scripts/check_repository.py`: passed (16 crates, 7 skills, links, UI tokens). `scripts/dev check`: passed.
- `cargo test -p yulius-app --lib skills`: 9 passed (embedded `infra-bootstrap` changed).
- Independent review (fresh subagent, diff + spec): request-changes, one medium (rule vs spec wording) fixed by aligning spec and docs with the check plus a test; low findings: `.gitattributes` added for LF; discovery dedupes by name (verified in skills.rs). Not fixed, accepted: empty directories and symlinks in deployments are not compared.
- Not run: full `scripts/dev ci` (Rust code untouched) and e2e; `apm audit` in CI (the pure-Python check covers it).
