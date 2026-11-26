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
`.gitattributes`, and three unclaimed paths: `spec_format/templates.rs` (the new-project
AGENTS.md and first ADR), `.ai/templates/spec.md` and the catalog text tests in
`crates/yulius-app/src/skills.rs`. AGENTS.md states no ADR rule besides the slot sentence, which
the third pull request removes.
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
  the slot machinery and the repository's union line for the index are gone, and a union merge
  of two branches that each add a record still passes the check in either order.
- **DOC-005** changes.md, architecture.md, operations.md and product.md fit the L10 Q9 budgets;
  moved text is moved verbatim to `docs/archive/` first; the briefing asks for a changes entry
  only for a user-visible change; the false sentences of Q5 are corrected.
- **DOC-006** A new spec's file name follows its heading, as far as this task's paths allow.
- **DOC-007** The README states the audience, what leaves the machine and what is not supported.

## Plan

- [x] ADR threshold in the briefing (rule 5), budget kept
- [x] ADR check for new ADRs in check_repository.py
- [x] ADR threshold in AGENTS.md, engineering docs and catalog skills
- [x] PR1 review approved
- [x] AGENTS.md: duplicated rules removed
- [x] docs/engineering: duplicated rules removed
- [x] .apm skills reduced to routing, apm install
- [x] Catalog skills: one source per rule, skills.rs text tests adjusted
- [x] PR2 review approved
- [x] Generated title-only ADR index
- [x] Slot machinery deleted
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
- The new ADR check reads the branch's additions against the merge base with
  `origin/$YULIUS_INTEGRATION_BRANCH` (CI checkouts share the sandbox clone's refs); with no
  such ref, and for a merge candidate (`YULIUS_CI_REF` `train/...`, several members already
  checked one by one), it checks nothing and says so.
- One source per rule (L10 Q1): where Q1 names code or the briefing as the source, the prose
  copies go and no text replaces them (rows 1-4); product-wide delivery rules stay once in the
  catalog skill that owns the step (`delivery-pipeline` for the local-check rule and the nit
  bound, `om-code-review` for the review rules, `test-driven-development` for mutation checks,
  `om-check-and-commit` for the quick checks, `git-hygiene` for branches, history and stashes);
  `.apm` skills only route. The skills.rs text tests assert the single source and the absence
  of every copy, so a copy cannot come back unnoticed. The briefing keeps its short forms (it is
  the run-time source and has its own budget tests).
- The ADR index is generated by a mode of `check_repository.py` (`--decision-index`), not a new
  script: number, title and a superseded mark read from the Status section; the check compares
  the committed file with the generated one. Yulius still adds a union line for the index to each
  clone's `info/attributes` (`integration_merges.rs`, outside this task's paths), so a merge of
  two new entries keeps both lines in either order. The check compares the entries as a set (no
  line missing, extra, duplicated or out of date), and `--decision-index` keeps the committed order
  (corrects lines in place, drops lines without a record, appends new ones), so a regeneration on a
  union-merged, unsorted index moves no line that a later union merge could duplicate. Left: two
  branches changing neighbouring lines (a title or Status mark beside a new record) can leave both
  versions, and the check asks for a regeneration; ADR-0403 says so.
- Briefing rule 5 stays within the budget in both decision layouts: the directory arm drops its
  section list (the project docs check still names missing sections; "Alternatives" is implied
  by "two rejected alternatives"), the file arm drops its parts list, and "with at least one
  Mermaid diagram" becomes "with a Mermaid diagram".
