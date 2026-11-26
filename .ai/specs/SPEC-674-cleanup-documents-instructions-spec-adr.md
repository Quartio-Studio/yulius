---
yulius-task: 65e5c2ed-bee3-4aa4-be3c-406cc6cbe133
status: in-progress
---

# SPEC-674: Documentation cleanup from audit L10 (thresholds, one source per rule, sizes, README)

## Goal

Carry out the fair documentation points of the November 2026 outside review (audit L10): fewer
ADRs and specs written for small choices, each rule stated once, documents an agent can read
whole, and a README that says plainly who Yulius is for and what leaves the machine.

## Context

The cleanup charter (attachment `audit-2026-11-25/CHARTER.md`) calls this a chore that needs no
spec. This spec exists only because the task has no workflow and its first pull request changes
Rust and Python: the Stop gate, the edit gate and `delivery_merge` require one for any
non-documentation change (`requires_spec`, `crates/yulius-domain/src/spec_format/mod.rs:713`),
which is finding F1 itself.

Findings carried out, in order, one pull request each (L10 §3): F4 (with the part of F1 inside
this task's paths), F2, F8, F3/F6/F9, F5, Q10. Not in scope: F7 (the owner decides), F10 (the
delivery task fixes it), renaming or moving existing specs and ADRs.

Paths: AGENTS.md, README.md, `docs/**`, `.apm/**` and its generated copies, `catalog/**`,
`briefing.rs`, `spec_format/mod.rs`, `scripts/check_repository.py`, the slot scripts,
`.gitattributes`, and the catalog text tests in `crates/yulius-app/src/skills.rs` (unclaimed).
The gates (`hooks.rs`, `discipline.rs`) and `crates/yulius/tests` belong to other tasks.

## Requirements

- **DOC-001** The briefing no longer asks to record each lasting decision as an ADR: an ADR is
  only for a choice about how the system is built or run that someone may want to reverse, with
  the alternatives considered. Its byte budget holds.
- **DOC-002** The repository check refuses a branch that adds an ADR without at least two items
  under `## Alternatives`, adds more than one ADR, or adds one while its other changes are only
  web UI files.
- **DOC-003** Each duplicated rule of L10 Q1 is stated in one place; rules the code enforces and
  explains at run time are not restated.
- **DOC-004** The ADR index is generated from the ADR files (number, title, superseded marker);
  the slot machinery and the union merge of the index are gone.
- **DOC-005** changes.md, architecture.md, operations.md and product.md fit the L10 Q9 budgets;
  moved text is moved verbatim to `docs/archive/` first; the briefing asks for a changes entry
  only for a user-visible change; the false sentences of Q5 are corrected.
- **DOC-006** A new spec's file name follows its heading, as far as this task's paths allow.
- **DOC-007** The README states the audience, what leaves the machine and what is not supported.

## Plan

- [ ] ADR threshold in the briefing (rule 5), budget kept
- [ ] ADR check for new ADRs in check_repository.py
- [ ] ADR threshold in AGENTS.md, engineering docs and catalog skills
- [ ] PR1 review approved
- [ ] AGENTS.md: duplicated rules removed
- [ ] docs/engineering: duplicated rules removed
- [ ] .apm skills reduced to routing, apm install
- [ ] Catalog skills: one source per rule, skills.rs text tests adjusted
- [ ] PR2 review approved
- [ ] Generated title-only ADR index
- [ ] Slot machinery deleted
- [ ] PR3 review approved
- [ ] changes.md cut to user-visible entries, briefing rule 6 changed
- [ ] architecture.md per-feature sections moved verbatim to docs/archive
- [ ] architecture.md trimmed to its budget
- [ ] operations.md: old upgrade notes and legacy unit sections archived
- [ ] product.md trimmed to current behaviour
- [ ] Q5 false sentences fixed
- [ ] PR4 review approved
- [ ] Spec file named from its heading (inside this task's paths)
- [ ] PR5 review approved
- [ ] README: audience, what leaves the machine, not supported
- [ ] PR6 review approved

## Acceptance criteria

- **AC-1** (DOC-001) The rendered briefing has no "record each lasting decision" and names the
  reversal and alternatives bar; every briefing budget test passes.
- **AC-2** (DOC-002) `check_repository.py` fails for an added ADR with one alternative, for two
  added ADRs, and for an ADR beside web-only changes; it passes an ADR with two alternatives and a
  branch that adds none.
- **AC-3** (DOC-003) No rule of Q1 rows 1-11 appears in more than its single source.
- **AC-4** (DOC-004) The index regenerates byte-identical from the files; the check fails on
  drift; `decision_slots.py` no longer exists.
- **AC-5** (DOC-005) `wc -l` of each document is within its budget; every local link resolves.
- **AC-6** (DOC-007) The README sections exist and match the code they cite.

## Test plan

`python3 -m unittest scripts/test_check_repository.py`, the `yulius-domain` briefing and
spec_format unit tests, the `yulius-app` skills tests, `scripts/dev quick` (repository contracts
and the link checker). The rest of the gate is Yulius CI's.

## Decisions

- The spec threshold (F1) is not changed in `requires_spec`: any path-based relaxation of
  `SpecPolicy::ByChange` fails gate tests in `crates/yulius/tests` (another task's paths), and the
  300-line part needs the gates to pass line counts. Listed under "Not done" in the pull request.
- The new ADR check reads the branch's additions against the merge base with `origin/dev`, the
  base the link checker's CI job already fetches; with no base it checks nothing.
