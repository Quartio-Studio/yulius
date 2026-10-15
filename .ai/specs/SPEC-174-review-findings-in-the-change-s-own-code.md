---
yulius-task: 5fa13a44-bb38-4bbc-8815-06d6b1de33dd
status: done
---

# SPEC-174: Review findings in the change's own code are fixed before merge, never deferred

Status: done
Date: 2026-10-15
Owner: Yulius task #164
Scope authorization: task #164 ("Tighten the delivery rules so that agents fix review findings in
the code their own change wrote before merge, instead of deferring them to a follow-up GitHub
issue"), from an audit of this repository's agent-filed issues.

## Goal

Every rule an agent reads about the review and follow-ups says the same three things, and none
lets an agent defer a finding in its own change's code to an issue:

1. A finding in code or tests the change added or modified is fixed in the same pull request,
   test-first, and never becomes a follow-up issue (a test that cannot fail, a weak or missing
   assertion, a race, a missing regression test included). The 3-round limit does not excuse it:
   keep fixing, split the change, or stop and ask a person.
2. Only a pre-existing bug, a flaky test the change did not cause, or a cosmetic nit left at the
   round limit becomes a new issue, which says why it is outside the change, with the evidence.
3. The reviewer classifies each finding as own code or pre-existing; the pull request answers every
   finding; deferring an own-code finding makes the verdict request changes.

## Context

- The 2026-10-15 audit: #524 (two checks in the task's own new e2e test cannot fail), #478
  ("recorded rather than fixed to close the review loop after five rounds"), #513 (left after round
  4), #477 and #436 (follow-ups on code the task had just written). These issues stay as they are.
- Where the rule lived, found by searching the repository for "follow-up", "rounds", "widening"
  and "accepts the risk":
  - `catalog/skills/yulius/delivery-pipeline/SKILL.md`: "Follow-ups, bugs you find and flaky tests
    become their own issues" and "At most 3 rounds; then decide, record the reasoning".
  - `catalog/skills/open-mercato/om-code-review/SKILL.md` and `references/review-checklist.md`: a
    major passes with "a recorded decision with its reason and a follow-up issue"; "after 3 rounds
    the author decides on the rest"; `references/output-format.md` has no classification.
  - `catalog/workflows/feature.md` ("Fix every finding test-first; at most 3 rounds") and
    `catalog/workflows/fix.md` ("record other problems you notice with `issue_create`").
  - `.apm/skills/yulius-review`, `yulius-verify`, `yulius-git`: silent on the rule; they review,
    verify and merge. `docs/engineering/workflow.md` (Review, Review output) likewise.
  - The briefing every task reads (`crates/yulius-domain/src/briefing.rs`): "after 3 rounds, decide
    and record why" and "File follow-up work, bugs and flaky tests as issues". The `issue_create`
    tool description (`crates/yulius-app/src/mcp.rs`) and its one-line summary in the briefing's
    tool list: "follow-up work, a bug or a flaky test".
  - SPEC-001 DLV-008: "Agents log follow-ups, bugs and flaky tests as issues".
  - AGENTS.md does not state the rule; it routes to `docs/engineering/workflow.md` and the skills.
- Catalog skills and workflows are embedded in the binary (`skills/pack.rs`, `workflows/catalog.rs`);
  the `.apm` sources generate `.agents/skills` and `.claude/skills` with `apm install`.

## Requirements

- R1. Each location above states the three points in its own words, briefly, and none keeps wording
  that lets an own-code finding be deferred.
- R2. The briefing's review line and issue line, its playground variant, and the `issue_create`
  tool description say the same.
- R3. A test fails if any location stops stating the rule or regains the old wording.
- R4. The generated skill copies match their `.apm` sources; the repository check passes.
- R5. ADR-0141 records the decision; SPEC-001 DLV-008 and `docs/changes.md` reflect it.

## Plan

- [x] Read the documents and the code they describe
- [x] Catalog skills and workflows state the own-code rule, checked by a test
- [x] The briefing and the issue_create tool state the own-code rule, with tests
- [x] Repository primitives, docs, SPEC-001 and ADR-0141 state the own-code rule
- [x] Gates green and independent review approved

## Acceptance criteria

- `skills::tests::review_findings_in_own_code_are_never_deferred` passes and fails on the old text
  of any listed file.
- The briefing tests assert the new review and issue wording and reject the old.
- `python3 scripts/check_repository.py` and `scripts/dev ci` pass.

## Test plan

- New unit test over the nine rule documents (whitespace-normalized, case-insensitive): required
  phrases present, deferral phrases absent. Watched failing before the edits.
- Briefing unit tests: the mandatory-text helper requires the new phrases; a test rejects "decide
  and record why" and "File follow-up work".
- `mcp.rs` tool-description test requires the narrowed `issue_create` wording.

## Decisions

- **Briefing and tool text are in scope.** The task names documents, but the briefing is where
  every task reads the rule first; leaving "after 3 rounds, decide and record why" there would keep
  the deferral allowed. Its text is product behavior, so it changes test-first.
- **A wording test instead of a merge check.** `delivery_merge` cannot tell an own-code issue from
  a pre-existing one without the reviewer's prose classification (ADR-0141 Alternatives).
- **Each location restates the rule briefly** rather than only linking, because the task asks every
  location to state the three points and agents often load only one skill.
- **The briefing stays inside its 9,500-byte budget** (SPEC-002 §7.5, Claude Code's 10,000-char
  hook limit). A first wording added 532 bytes and cut optional sections in
  `huge_inputs_respect_the_budget_and_keep_mandatory_parts`, then one decision line in
  `short_parts_donate_budget_to_long_ones`. Rather than relax either test, the issue rule is folded
  into the existing "Report a bug you confirm" sentence and the review line is terse; the full
  wording lives in the skills and `docs/engineering/workflow.md`.
- **Workflow instructions count against the same budget.** The session hook subtracts a task's
  workflow instructions from the 9,500 bytes (`hooks.rs`), and SPEC-173's test models them as at
  most 1.7 KB. The first wording grew the Fix workflow from 1,394 to 2,031 bytes, which would have
  cut the briefing's mandatory tail (the architecture pointer, an isolated sandbox's Containers
  section) in every Fix task. The workflows now state the rule in one terse sentence (Fix: 1,683
  bytes with its heading; Feature: 1,415), Fix's "record other problems you notice with
  `issue_create`" folds into it, and `the_shipped_catalog_is_valid` fails any bundled workflow over
  1,700 bytes. The Git rules grow by at most the 39 bytes of slack dev left in
  `the_containers_section_never_displaces_the_task_or_the_architecture`.

## Evidence

- `skills::tests::review_findings_in_own_code_are_never_deferred` failed before the edits
  ("delivery-pipeline/SKILL.md does not say \"own code\""), passes now; its pre-existing-major
  assertion failed before the round-1 fix. The briefing and `issue_create` tests failed on the old
  text, then passed; `the_shipped_catalog_is_valid` failed at 1,713 bytes (Feature) before the
  workflows were shortened.
- `scripts/dev ci`: exit 0, 1849 tests passed (at 51a91ca0 and at 59888ce1); rerun on the final
  head before delivery.
- `python3 scripts/check_repository.py`: passed. `apm install` (apm-cli 0.33.0): `.agents/skills`
  and `.claude/skills` match `.apm`, `apm.lock.yaml` updated.
- `scripts/dev security`: **unrun**, exit 2, "Vidoc rejected the key (HTTP 401)" (#470,
  ADR-0104); the CI `security` job reports it skipped.
- e2e: not run; no web UI change.
- Independent review in fresh sessions (`om-code-review`): round 1 (51a91ca0) requested changes,
  with one major (the verdict gave no outcome for a pre-existing major), three minors and four nits,
  all in this change's own code and all fixed test-first. Round 2 (59888ce1) approved and listed one
  minor (this Evidence section) and three briefing nits, all own code and fixed. Merging dev then
  exposed the budget problem above, also own code, also fixed. A confirmation round reviews the
  final head.
- Pre-existing, filed: #556 (Feature and Fix workflows still say "promote, verify production",
  present on dev before this change).

