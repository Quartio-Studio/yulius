---
yulius-task: 12ae73c7-bbed-467a-846c-24ed2d207a5d
status: done
---

# SPEC-517: a task that finds a bug fixes it

## Goal

A request that reports something wrong in the product (a bug, an error, a broken, missing or
confusing behaviour, a screenshot of a fault) leads to the fix by default, also when it is phrased
as a question ("What", "why is this broken"). A task that confirms such a defect while answering
goes on to fix it in the same task (its plan grows the fixing steps), or asks the person on the task
whether to fix it now when the fix is clearly outside the request. It never files an issue and
stops. True questions ("how does X work", "what would Y cost") still get an answer without a pull
request. The owner's words (2026-11-19, about quartio-v2 task #312): "I sent an task and it spawned
an issue instead of fixing this. [...] spawn a task in yulius to fix this in yulius so it won't
happen again".

## Context

**What happened in quartio-v2 task #312** (created 2026-11-19T11:55:30Z, harness claude, model
claude-opus-5-5, branch `feat/312-some-weird-problem-with-pdf-i-couldnt-at`): the prompt was a
screenshot and 'some weird problem with pdf "i couldnt attach" but attached. What'. The task's plan
was "Read the product context and earlier work on documents and PDFs", "Trace where the PDF export
fails and why the Word file still attached", "Answer with the cause, evidence and the fix to start".
It reproduced a real product bug on the dev server, filed Quartio-Studio/quartio-v2#1392 with the
cause and acceptance criteria, answered in words and stopped at 3/3 steps: no spec, no test, no
fix, no pull request. The owner had to start #313 by hand.

**The cause, from the code that task ran through.** A task cannot read another sandbox's sessions
(SPEC-335: only the person's Superagent reaches them; `GET /api/sandboxes` answers 404 to a task
token), so the evidence is the plan quoted in this task's request, set against this repository:

1. *The plan is the "Research / Question" workflow's.* Its steps (`catalog/workflows/research.md`)
   are "Read the product context, docs and earlier sessions on the topic", "Investigate the code,
   data or sources and collect evidence" and "Answer with the evidence, the options and a
   recommendation": #312's three steps are these three, worded for PDFs. A task created with a
   workflow starts with the workflow's steps (`tests/auto_choice.rs`
   `questions_run_as_prompts_without_a_spec_first_workflow`).
2. *That workflow tells the agent to file and stop.* Its instructions, put into the briefing as
   "Workflow instructions", say "The deliverable is an answer, not a change", "Change no code. When
   the answer reveals work, record it as an issue (`issue_create` …) or tell people which Feature or
   Fix task to start". #312 did exactly that.
3. *Auto sends a question-shaped bug report there.* The composer's mode and workflow default to
   Auto (TSK-015). Jev's mode question offers only `goal` ("build, change, fix…") or `prompt` ("a
   question, research… no change to the product is asked for"), and its workflow question says
   "research, questions about the product, explanations and reviews [need no spec], so never pick a
   spec-first workflow for them". A prompt ending in "What" reads as a question; nothing says that a
   reported fault is a fix whatever its phrasing. Auto also never gives a prompt-mode task a
   spec-first workflow, so even a Jev pick of Fix would end with no workflow there.
4. *The briefing backs the issue.* Every task's briefing says "Report a bug you confirm with
   `issue_create` unless an open issue tracks it", with no word on fixing a bug the request itself
   reports, and "Work that changes no file ends with its result".

The owner's Superagent read #312 for this task (`sandbox_task_get`, 2026-11-19): three steps,
all done, the second with the note "Reproduced on dev: soffice exit 77 as uid 997 (no passwd
entry); issue #1392". The agent had confirmed the defect, then filed it, as the workflow said. The
cross-sandbox read does not show the mode, the workflow or the `auto` line, so whether Auto or the
person picked "Research / Question" stays unknown. The change below closes both paths: Auto no longer routes a reported defect to a question workflow, and the
question workflow itself no longer ends a confirmed defect in an issue.

**Related work.** TSK-015 and SPEC-002 ADR-16 item 5 (Auto mode and workflow), the workflow catalog
(SPEC-001 AUT-006, `catalog/workflows/`), SPEC-174 / ADR-0141 (`issue_create` only for what lies
outside the change), the briefing (`crates/yulius-domain/src/briefing.rs`, 9,500-byte budget).

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| FIX-001 | Auto's mode question offers a third answer, `fix`: the request reports something wrong in the product (a bug, an error, a broken, missing or confusing behaviour, a screenshot of a fault), also when it is asked as a question. A confident `fix` runs the task as a goal (`goal` and `fix` together count as Jev's confidence of Goal); `goal` and `prompt` keep their meaning, and a true question (how something works, what it would cost) is a `prompt`. | domain unit test, `tests/auto_choice.rs` |
| FIX-002 | For a task whose mode answer is a confident `fix`, Auto's workflow is never one that needs no spec: Jev's pick of such a workflow (Research / Question), or an unsure answer, becomes the workflow named "Fix" (the sandbox's own first), else the default; Jev's spec-first pick and its confident "none" are kept. The task therefore starts with the fixing steps. The audit detail says the request reports a defect. | domain unit test, `tests/auto_choice.rs` |
| FIX-003 | The workflow question says that a request reporting something wrong in the product is a fix also when asked as a question, never a research or question workflow. | domain unit test |
| FIX-004 | The "Research / Question" workflow is for questions that report nothing wrong, and its instructions say: a defect the request reports, or one the investigation confirms in what it asks about, is fixed in the same task (the plan grows the Fix workflow's steps); when the fix is clearly outside the request, or it is unclear whether the person wants an answer or a fix, the agent asks with `needs_you` instead of guessing; it never files the defect as an issue and stops. Only a second, unrelated defect becomes an issue, and the answer names every issue opened. The "Fix" workflow's "when to use" covers a fault asked as a question or shown in a screenshot. | catalog test |
| FIX-005 | The briefing says the same in the line on `issue_create`: a defect the request reports, even asked as a question, is fixed in the task; each confirmed bug is still reported (DSC-016), and one met on the way is named in the summary. The `issue_create` tool's description says the defect the task's own request reports is fixed in the task, never filed instead (the issue the Fix workflow opens for it closes with `Fixes #<number>`), and that every issue opened is named in the summary; the Fix workflow's own reporting rule is asserted beside it. | briefing test, MCP test |

## Plan

- [x] Read #312's evidence and the code paths it ran through, then write and validate the spec
- [x] Auto: a defect asked as a question runs as a goal with the Fix workflow's steps
- [x] Research / Question and Fix workflows: a confirmed defect grows fixing steps or asks the person
- [x] Briefing and issue_create: fix the request's defect, file only what lies outside it, name it
- [x] ADR-0371, product, API and SPEC-001 TSK-015 say the rule; changes log entry
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1** (FIX-001, FIX-002) A task created on Auto with the prompt
  `some weird problem with pdf "i couldnt attach" but attached. What?`, for which Jev answers mode
  `fix` and workflow "Research / Question", runs as a goal with the workflow "Fix" and its steps
  are the Fix workflow's steps (reproduce, regression test, fix, review); its `auto` says the
  workflow was set by the rule (`default`).
- **AC-2** (FIX-001) A true question on Auto ("How does the subscription pool pick an account for a
  new task?"), for which Jev answers `prompt` and "Research / Question", still runs as a prompt
  with the Research steps and no fixing step.
- **AC-3** (FIX-002) With mode `fix` and a confident spec-first pick (Fix, or the sandbox's own
  spec-first workflow) or a confident "none", Auto keeps Jev's pick; unsure gives Fix; no workflow
  named Fix gives the default (Feature).
- **AC-4** (FIX-004) The Research / Question workflow's instructions no longer say "Change no code.
  When the answer reveals work, record it as an issue"; they say a confirmed defect is fixed in the
  same task or asked about with `needs_you`, and they stay within the briefing's workflow block
  limit.
- **AC-5** (FIX-005) The briefing and the `issue_create` description carry the rule and the
  briefing stays within its budget tests.

## Test plan

- Domain (`crates/yulius-domain/src/auto_choice.rs`): `fix` in the mode question and its texts;
  `mode_choice` and `reports_defect` for `fix`, `goal`, `prompt`, unsure; `workflow_choice` with a
  defect for a no-spec pick, unsure, a spec-first pick, "none", and no Fix workflow.
- Integration (`crates/yulius/tests/auto_choice.rs`): the fake Jev answers `fix` for a request that
  reports a problem; AC-1 and AC-2 through the public API, with the steps compared to the
  workflows API's lists.
- Catalog (`crates/yulius-app/src/workflows/catalog.rs`): AC-4 phrases present and absent.
- Briefing (`crates/yulius-domain/src/briefing.rs`) and MCP (`crates/yulius-app/src/mcp.rs`): AC-5.
- What cannot be tested here: how the real Jev answers the new mode question (a fake stands in, as
  in every Auto test), and whether a real agent follows the reworded instructions; both are text
  given to a model.

## Decisions

- **A third mode answer, not a keyword rule.** Telling a reported fault from a question by words
  ("broken", "error", "couldn't") would be brittle across languages and phrasings; Jev already
  reads the request, so the question it answers now names the case. `fix` maps to Goal mode, so
  nothing else in the task model changes (ADR-0371).
- **Only no-spec workflows are overridden.** A confident "none" or a spec-first pick (a sandbox's
  own bug workflow, Feature for a confusing behaviour) already lets the agent fix it; the harm is a
  workflow whose instructions say not to change code.
- **Ask, do not guess.** When the agent cannot tell whether the person wants an answer or a fix, or
  the fix is clearly outside the request, it asks with `needs_you` (decision) and answers in the
  meantime; the person on a question-shaped task is there to answer.
- **Review round 1.** The texts now agree with the Fix workflow and DSC-016: the request's own bug
  is fixed, never filed *instead*, and the issue opened for it closes with the fix. A goal/fix
  split counts both for Goal. A picked mode (no mode question) and a Docs or Chore pick for a
  reported fault are covered as ADR-0371's Consequences say.
- **The Fix workflow still reports the bug it fixes** (`issue_create`, then `Fixes #N`): that issue
  closes with the merge, so it does not add to the open list.

## Evidence

- Cause (task #312): its plan is the Research / Question workflow's three steps; that workflow said
  "Change no code. When the answer reveals work, record it as an issue"; its step 2 note (read by the
  owner's Superagent) "Reproduced on dev: soffice exit 77 as uid 997 (no passwd entry); issue #1392".
  Mode, workflow and the `auto` line are not readable across sandboxes, so the change covers Auto
  and a hand pick.
- Tests written first and seen failing: `auto_choice::a_defect_asked_as_a_question_gets_the_fixing_steps`
  (Research steps where Fix's were expected), `workflows::catalog::tests::a_defect_found_answering_a_question_is_fixed_not_filed`,
  the new briefing and `issue_create` needles, the goal/fix split case in
  `auto_choice::tests::a_reported_defect_is_a_goal_and_both_questions_say_so`.
- `cargo nextest run -p yulius-domain -p yulius-app`: 1,185 passed.
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^auto_choice::/) | test(/^skills/) | test(/^agent_hooks::/))'`:
  all passed. `scripts/dev quick`: repository contracts and `cargo fmt --check` passed.
- Briefing budget: the three budget tests (`short_parts_donate_budget_to_long_ones`,
  `a_long_goal_shrinks_before_a_mandatory_part_is_cut`,
  `the_containers_section_never_displaces_the_task_or_the_architecture`) failed with the first
  wording (+128 bytes) and pass with the final one, paid for by a shorter header line.
- Research / Question's instruction block: 1,375 of 1,444 bytes.
- `yulius security-scan`: exit 2, not run (Vidoc refused the key, HTTP 401, an instance-wide
  condition); CI's `security` job skipped (exit 78). Not a pass.
- Review round 1 (Fable 5.2): request changes, one major (the texts forbade the issue the Fix
  workflow opens for its bug) and minors; all fixed test-first in a8e654225.
- Review round 2 (Fable 5.2): request changes, a `clippy::collapsible_if` blocker in
  `mode_choice` and minors (Research's headline, SPEC-002 ADR-16 item 5, comments); fixed.
  `cargo clippy --locked -p yulius-domain -p yulius-app -p yulius --all-targets --all-features --
  -D warnings`: passed. The briefing keeps no "(reference it)" for want of bytes; the Fix
  workflow still says to reference the tracking issue.
- The whole gate: Yulius CI on the pull request (#1430) head.
- Not testable here: how the real Jev answers the three-way mode question, and whether agents
  follow the reworded instructions; both are text given to a model.
