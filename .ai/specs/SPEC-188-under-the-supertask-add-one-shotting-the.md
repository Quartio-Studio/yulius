---
yulius-task: e6f60efa-0ed5-494a-8c00-b829a33a3748
status: in-progress
---

# SPEC-188: Supertask names one-shotting the product, and Jev proposes it for broad work

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #173
Scope authorization: task #173, "Under the supertask add '...one-shotting the product etc' (make
sure grammar is correct)", with a screenshot of the start menu's Supertask option; and the
person's follow-up in the same task: "also make sure jev can stop the user and propose this type
when different is used but the work is really really broad. But for example 'fix all github
issues' are not in this category because the task will split it". Amends the wording of SPEC-094
MDS-004 and extends task intake (SPEC-001 TSK-011, TSK-018; SPEC-002 ADR-16).

## Goal

1. The start menu's Supertask option names a second example of broad work beside security
   hardening: building the whole product in one go ("one-shotting the product"), in correct
   English.
2. When a person starts a new task another way (Start now, Run after, a rule) but the request is
   really broad work, Jev stops the start and the composer proposes running it as a supertask.
   A request that splits into separate tasks (such as "fix all GitHub issues") is not broad.

## Context

- **Where the text lives.** `startchoice.js` `SUPERTASK_HELP` is the option's description in the
  start menu and the start chip's tooltip on a new task (`taskview.js`). Today it reads "Waits for
  this project’s running tasks, then runs alone while the project’s other tasks wait. For broad
  work such as security hardening." (SPEC-094 MDS-004, pinned by `menus.e2e.ts`).
- **The menu's height budget.** At 1280 × 800 the start menu opens above the chip and just fits
  (SPEC-102: "the whole menu does not scroll at this height"). Appending "or one-shotting the
  product" makes the description a fourth line (64 px instead of 48 px) and the whole menu scrolls
  by 5 px, clipping "When a rule is true…" — `menus.e2e.ts` SPEC-102 caught it. Measured in the
  real menu, only rephrasings of about 133 characters stay at three lines.
- **Same example elsewhere.** The `task_create` MCP tool's description (`mcp.rs`) and
  `docs/product.md` say a supertask is "for broad work such as security hardening".
- **Task intake today.** `POST /api/sandboxes/{sid}/intake/check` asks Jev (TypeSafe's
  `jev-1.13.0`, one fixed Choice question per request, fixed thresholds) whether a new task's
  prompt of 40+ words mixes separate workstreams (`SPLIT_QUESTION`, `one`/`several` at 0.9). The
  composer calls it on Start only when no start choice was made; on `several` it shows the split
  card. Jev failures, no key and short prompts all mean "create as asked". Auto mode and workflow
  ask two more questions at once when the task is created (`auto_choice.rs`).
- **Supertasks** (TSK-018, SPEC-071, ADR-0054) wait for the project's working tasks, then run
  alone; a supertask cannot also run after a task or a rule (API 422), nor be prepared.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OSP-001 | The start menu's Supertask description (and the new task's start chip tooltip) reads: "Runs alone once this project’s running tasks end; its other tasks wait. For broad work: security hardening, one-shotting the product." It shows whole at 1280 px and 390 px (SPEC-094) and the start menu still fits a 1280 × 800 window without scrolling (SPEC-102). | e2e `menus.e2e.ts` SPEC-094 and SPEC-102 tests |
| OSP-002 | The `task_create` tool's description and `docs/product.md` name the same examples: "for broad work such as security hardening or one-shotting the product". | `mcp.rs` unit test |
| OSP-003 | Jev answers a fixed `breadth` question about a new task's request: `focused` or `broad` (work across most of the product at once that should run alone, such as security hardening, one-shotting the product, a project-wide refactor, migration or upgrade). The question says that a list of separate items, bugs or issues ("fix all open GitHub issues") is not broad: it splits into separate tasks. | domain unit test on the question |
| OSP-004 | `/intake/check` answers `"supertask": true` only when Jev is confident (≥ 0.85) the request is `broad` **and** the split verdict is not `several` (a request that splits is never proposed as a supertask). Prompts under 3 words, no Jev key and every Jev failure answer `false`. The breadth and split questions are asked at once, each within the 2.5 s check deadline; `checked` says whether Jev answered either. | domain unit tests; `tests/intake.rs` |
| OSP-005 | When a task is started from the composer with Start now, Run after or a rule, the composer runs the check; on `supertask` it does not create the task and shows a card in the split card's place: "Run it as a supertask?", why, and — when a task or rule was chosen — that the supertask replaces it. **Run as a supertask** creates it as a supertask (without the task or rule); **Start as chosen** creates it as the person chose; the close button withdraws and keeps the prompt. A task started as a supertask or prepared ("When I decide", "At a time…") is never stopped. A request that gets the split card never also gets this card. No axe violations. | e2e `intake.e2e.ts` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The Supertask description names one-shotting the product, in the menu, the tool and the docs
- [x] Jev answers whether a new task is broad work, in the intake check
- [ ] The composer stops a broad task started another way and offers Supertask
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- New task's start menu shows Supertask with "…For broad work: security hardening, one-shotting
  the product." wholly visible, and the menu fits a 1280 × 800 window.
- An agent's `task_create` tool description names the same two examples.
- With Jev configured, "Harden the security of the whole product: …" started with Start now
  shows "Run it as a supertask?"; Run as a supertask creates a supertask; Start as chosen creates a
  normal task. "Fix all open GitHub issues" (a list of separate items) is not proposed as a
  supertask, and a request Jev splits shows only the split card.
- Without Jev, or when it fails, every task starts as chosen, with no card.

## Test plan

- `e2e/tests/menus.e2e.ts`: the SPEC-094 test's expected text changes first and fails on the
  unchanged menu; SPEC-102's test guards the height budget.
- `crates/yulius-app/src/mcp.rs`: a unit test that `task_create` names both examples.
- `crates/yulius-domain/src/intake.rs`: unit tests for `BREADTH_QUESTION` (options, the "fix all
  GitHub issues" rule in its text) and `supertask_proposed` (confident broad; several wins; an
  unsure or disagreeing answer is not broad; short prompts skip).
- `crates/yulius/tests/intake.rs`: `/intake/check` with the fake Jev: a broad request answers
  `supertask: true`; a broad request that also splits answers `several` and `supertask: false`;
  a short prompt, Jev down and no Jev answer `false`; the existing exact-body assertions gain the
  field.
- `e2e/tests/intake.e2e.ts`: the card on Start now (no axe violations), Run as a supertask creates
  a supertask, Start as chosen a normal task, Run after shows that the supertask replaces it, the
  close button keeps the prompt, a chosen Supertask is never stopped; the mock answers by
  keywords as the fake Jev does.
- Gates: `scripts/dev ci`, the e2e suite, `scripts/dev security`.

## Decisions

- **Menu wording kept at three lines.** The literal addition ("…such as security hardening or
  one-shotting the product.") is a fourth line that makes the whole start menu scroll at
  1280 × 800 (SPEC-102). Options measured in the menu: the literal text (4 lines), a dozen
  rephrasings with "such as"/"like" (all 4 lines), shortening another option's description (out
  of scope), and "Runs alone once this project’s running tasks end; its other tasks wait. For
  broad work: security hardening, one-shotting the product." (3 lines). Chosen: the last. It
  still says whose tasks wait (MDS-004), and the colon introduces examples. The request's "etc" is
  dropped: a list of examples after "for broad work" already says it is not complete, and "such
  as … etc." would be redundant. The tool description and docs, without a width limit, use "such
  as security hardening or one-shotting the product".
- **One more fixed Jev question, not a third split verdict.** Options: (a) a third `SPLIT_QUESTION`
  option `broad`; (b) a separate `breadth` question asked at once with the split question; (c) a
  rule on keywords. (a) mixes two independent properties in one distribution and would change the
  tuned split behavior; (c) is brittle. Chosen: (b), the pattern of the Auto questions (ADR-16:
  one fixed question per request, fixed thresholds). Recorded in ADR-0152.
- **A split wins over breadth.** The person's rule: work the task splits ("fix all GitHub
  issues") is not supertask work. So `several` always suppresses the proposal, and the question
  itself tells Jev that a list of separate items is focused.
- **Threshold 0.85, fixed.** Between the scope (0.8) and split (0.9) thresholds: the card stops a
  start, so it should be confident, but "really really broad" requests are rare and should still be
  caught. No setting, as the Auto thresholds have none; a setting can come when someone needs to
  tune it.
- **Prepared tasks are not stopped.** Their start is decided later by the person (Start now or a
  time), and a supertask cannot be prepared (API 422): proposing one would discard that choice.
  Run after and a rule are replaced by the supertask, and the card says so.
- **The check now runs for short prompts too.** Broad requests are often short ("Harden the
  security of the whole product"). The breadth question needs 3 words; the split question still
  needs 40. Without a Jev key the server answers at once.
