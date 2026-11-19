# 0371. A defect a request reports is fixed, also when asked as a question

## Status

Accepted on 2026-11-19 (`F 2026-11-19 Fix not file`: the owner, about quartio-v2 task #312, "I
sent an task and it spawned an issue instead of fixing this. [...] spawn a task in yulius to fix
this in yulius so it won't happen again"). Specified in
[SPEC-517](../../.ai/specs/SPEC-517-a-task-that-finds-a-bug-fixes-it-a-quest.md). Amends TSK-015
(Auto mode and workflow) and the workflow catalog; [ADR-0141](0141-review-findings-in-the-change-s-own-code-are-fixed-before-merge.md)
stands.

## Context

- Quartio-v2 task #312 was a screenshot and 'some weird problem with pdf "i couldnt attach" but
  attached. What'. Its plan was the "Research / Question" workflow's three steps, worded for PDFs.
  It reproduced the bug on the dev server (`soffice` exit 77 as a uid without a passwd entry),
  filed issue #1392 and stopped at 3/3 steps with no fix. The owner had to start #313 by hand.
- That workflow's instructions said "Change no code. When the answer reveals work, record it as an
  issue". Auto's mode question offered only `goal` or `prompt` ("a question … no change to the
  product is asked for"), so a fault phrased as a question read as a question, and Auto never gives
  a prompt-mode task a spec-first workflow. Every briefing said "Report a bug you confirm with
  `issue_create`", with no word on fixing the bug the request itself reports.
- The owner writes short prompts that often read like questions; tasks filing their bugs as issues
  left the project with about 300 open issues nobody reads.

## Decision

- **Auto asks about a fault.** Jev's mode question has a third answer, `fix`: the request reports
  something wrong in the product (a bug, an error, a broken, missing or confusing behaviour, a
  screenshot of a fault), also when asked as a question. A confident `fix` runs as a goal; `goal`
  and `fix` together count as Jev's confidence of Goal. Its
  workflow is never one that needs no spec: Jev's pick of such a workflow (Research / Question), or
  an unsure answer, becomes the workflow named "Fix" (the sandbox's own first; else the default
  Feature). Jev's spec-first pick and its confident "none" stand. The workflow question says the
  same.
- **A question task fixes what it confirms.** The Research / Question workflow is for questions that
  report nothing wrong. A defect the request reports, or one the investigation confirms in what it
  asks about, is fixed in the same task: the Fix workflow's steps join the plan. When the fix is
  clearly beyond the request, or it is unclear whether the person wants an answer or a fix, the
  agent asks with `needs_you` (a decision) and answers meanwhile. It never files the defect
  instead of fixing it; an issue opened for it (as the Fix workflow does, DSC-016) closes with the
  fix's `Fixes #N`.
- **Filing stays for what lies outside.** A second, unrelated defect met on the way (a flaky test,
  a fault outside the task) is still an issue, and the task's summary names it. The briefing, the
  `issue_create` description and the `delivery-pipeline` skill say so. The Fix workflow still files
  the bug it fixes and closes it with `Fixes #N`.
- **True questions stay questions.** "How does X work" and "what would Y cost" are `prompt`, run with
  the Research steps and end with an answer, no pull request.

## Alternatives

- **A keyword rule** ("broken", "error", "couldn't" → fix): brittle across phrasings and languages,
  and wrong for "why is the build slow?". Rejected; Jev reads the request already.
- **Turn every question into a goal**: true questions would end in pull requests nobody asked for.
  Rejected.
- **Override Jev's "none" too**: a goal without a workflow already fixes (the briefing's rule);
  only a workflow that says "change no code" did harm. Rejected.
- **A stop check that refuses an end after `issue_create`**: a question task that rightly filed an
  unrelated defect would be blocked, and the agent's own judgment of scope is what the rule needs.
  Rejected; the instructions carry the rule.

## Consequences

- A question-shaped bug report on Auto starts with the Fix workflow's steps and ends in a pull
  request. A question task that confirms a defect grows its plan, or asks the person.
- A task whose person picked "Research / Question" by hand, or whose mode was picked (the API's
  default Goal, an agent's `task_create` with only the workflow on Auto, so no mode question is
  asked), gets the rule through the workflow question's text and the workflow's instructions,
  not through the override.
- A task that stays on Research / Question and grows the Fix steps keeps that workflow's gates
  (spec and tests optional, DSC-013): its spec and regression test rest on the instructions
  (`om-fix`), not on a gate. Auto's route for a reported fault is the Fix workflow, which gates
  them.
- Docs and Chore need no spec either, so a confident Docs pick for a reported fault ("the README's
  install command is wrong") also becomes Fix, with its spec and regression test: accepted, since
  the request reports something wrong and the Fix path is the one that proves it fixed.
- How the real Jev splits `goal`, `fix` and `prompt` is not tested here: a fake stands in, as in
  every Auto test. Whether agents follow the instructions is model behaviour.
- The briefing's header is shorter ("Yulius coordinates this repository's agents by the discipline
  below") to keep the briefing within its 9,500-byte budget.
