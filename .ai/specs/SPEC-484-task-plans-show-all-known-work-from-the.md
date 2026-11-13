---
yulius-task: 3936a7cc-f429-472d-8370-43b2e879bf90
status: in-progress
---

# SPEC-484: a task's steps show all the work it knows of

Status: in-progress
Date: 2026-11-13
Owner: Yulius task #466
Scope authorization: the request of task #466 (by MrFishPL, 2026-11-13): "A task's step count is
not credible: it often reads complete (or nearly) while much of the work the agent already knows
about is still ahead, and the number of steps grows over time. Fix it, so the step strip and the
percentage a person sees say how far the whole known work is." Amends SPEC-001 STP-006, STP-008,
GIT-001 and DLV-008 and SPEC-002 ADR-19; decision in
[ADR-0325](../../docs/decisions/0325-a-pull-request-delivers-the-finished-part-of-a-plan.md).

## Goal

The step strip and the percentage of a task say how far the whole work is that its agent knows
of. A task that delivers fifteen slices shows fifteen from the moment it knows them, a pull
request of one slice merges while the later ones are still ahead, and a plan that grew says so
to the person.

## Context

- **What the person saw (2026-11-13).** Task #465 read "Step 6/6" with a full bar while its spec
  (SPEC-483, Context) listed 15 slices, 5 merged. Earlier that night it read 4/4, 2/2, 3/3, 4/5,
  5/6, 6/6. The person: "You say 5 of 15 but I see only 6 steps. It is happening often that the
  plan isn't so credible and the number of steps grows in the time."
- **The cause, from #465's transcript.** A merge is refused while any step is open
  (`yulius_domain::delivery::check_merge`: "N step(s) are still open; finish or skip them before
  merging"), and so is the push and the pull request before it (`try_auto_open_pr`: "step … is
  pending: finish it, or resolve it with `step_update` and a note"). The rule assumes one pull
  request per task. #465 delivers one pull request per slice. At 02:19 UTC, with slice 1 armed
  to merge, it added slice 2's four Plan items; `delivery_status` answered "Open steps: 4 …
  merge: 4 step(s) are still open". It took them out, and wrote the workaround into its spec
  ("a slice is ticked when its change is reviewed and ready to merge, and the next is added when
  it is merged") and into the agent's memory ("park next-slice items in `target/`"). From then
  on the plan held only what was already finished or under way: n/n before every merge. Today
  it reads 11/11, 100 %, with slices 12 to 18 named in its spec.
- **How common, from the transcripts of 385 tasks** (the results of `steps_set`, `step_update`
  and `steps_get`, in time order; `target/evidence/step-history.txt`): the count of steps grew
  in 184 tasks (48 %), by three or more in 65, to at least double in 31. In 69 tasks (18 %) it
  grew right after the plan read n/n or (n-1)/n, in 34 of them from n/n. Of 613 steps added
  later, 410 replaced a seed plan before anything was done (harmless), and of the rest the
  largest groups are review rounds ("Review round 2 answered", 252 with the next two groups
  overlapping), merges and gates ("Merged into dev on green CI", 86) and the next slice or
  delivery ("Second delivery: …", "After the merge: …").
- **The forms.**
  1. *The next slice after the merge* (#465; #452's "Second delivery"; #449's and #453's "After
     the merge: …"): the work was known, and the delivery rule kept it out of the plan.
  2. *A review round per step*: the plan ends with "Independent review answered", it is ticked
     when round 1 is answered, and round 2 adds "Review round 2 answered" to a plan that read
     n/n. The review of the fix is known work when the step is ticked. #438 went from 9 steps
     to 16 this way, eight times from n/n; #432 from 6 to 18.
  3. *Work truly found later* (a regression, a merge-blocking race): a new step is right, and
     the person cannot tell it from the first two.
- **What the instructions say.** The briefing's rule 3 asks for "3-12 steps of similar effort"
  and says "new work gets a new step". The same range is hand-written in the scaffolded
  AGENTS.md (`spec_format/templates.rs`), the `delivery-pipeline` and `om-spec-writing` skills
  and SPEC-001 STP-006; the Feature workflow says "3-10 slices". A plan of more than 12 steps
  is nudged: "Merge related steps into 3-12 slices of similar effort; tiny steps make the bar
  jump." A plan has at most 30 steps. Nothing says that the plan holds all the known work, and
  an agent with 18 slices is told to merge them into 12.
- **What exists to build on.** A task already delivers again after its pull request merged
  (SPEC-425, ADR-0269): work committed after the merge becomes a new pull request. A task with
  open steps is not Done, whatever its pull request (`AttentionFacts::plan_open`, #139), and
  tasks chained after it wait (`task_is_done`). Nested Plan items are kept in the spec and are
  not steps (`sync_steps_from_spec` takes depth 0), so a slice's own parts already have a place.
  Every change of a step's status is in `step_events`; a replaced plan's removed steps are not.
- **How "Step n/m" is made.** `steps::progress` counts done and skipped steps, weighted by
  effort; the strip shows the ordinal of the step in progress over the total and the bar the
  finished share plus the share in progress (`taskview.js`), the sidebar the same count
  (`status.js`), and the tools "Progress: d/t steps done, p% by effort". The computation is
  right: what is wrong is the total.

## Requirements

- PLN-001 (a pull request delivers the finished part). `check_merge` no longer refuses a merge
  for steps that are not finished. It refuses only a plan none of whose steps is finished
  (`DeliveryError::NoStepFinished`: "no step of the plan is finished; mark done what this pull
  request delivers (`step_update`) before merging"). A plan without steps is as before.
  `DeliveryFacts` carries `steps_open` and `steps_finished`. The same holds for a playground's
  squash.
- PLN-002 (what `delivery_merge` waits for). The delivery a `delivery_merge` call makes (push,
  pull request, a new pull request after a merge) goes out with steps still ahead. It is
  refused while a step is in progress ("step "…" is in progress: finish it if this pull request
  delivers it, or set it back to pending (`step_update`) when it belongs to a later one") and
  while no step is finished. The delivery at the end of a turn stays as it is: it waits for
  every step, so a turn that ends between two steps opens no pull request. Its reason for a
  plan with finished steps and others ahead names the way: "N step(s) are still ahead (next:
  "…"): Yulius delivers by itself once every step is finished; to deliver the finished part
  now, call `delivery_merge` with its reviewed head".
- PLN-003 (the agent hears what is ahead). The answers of `delivery_merge` (merged, armed,
  armed already) and the message about a merged pull request (`merged_text`) name the steps
  still ahead and the next one, and say that the task goes on with them on this branch; with
  none ahead they are as before. A merged task with steps ahead is not Done and the tasks
  chained after it wait (existing rules, pinned here by a test).
- PLN-004 (a plan holds every slice). A plan has 1 to 60 steps (`steps::MAX_STEPS`; the spec's
  Plan, `steps_set` and a workflow's seed steps share the bound). The "too fine" nudge
  (`Nudge::FinePlan`) is removed: more than 12 steps is what a task of several pull requests
  has. `steps::PER_PULL_REQUEST` (12) is the recommended most for one pull request, in the
  texts only.
- PLN-005 (a complete plan asks once more). When every step is finished, the nudges hold
  `Nudge::Complete`: "Every step is finished, so the task reads 100 %. If you know of work that
  is not done (a later slice or pull request, the review of a fix), add its steps now
  (`steps_set`)." It comes with the step tools' results and the briefing like every nudge.
- PLN-006 (the briefing). Rule 3 says: plan all the work known now, later slices and pull
  requests included, 3-12 steps per pull request and at most 60; a step is done when nothing of
  it remains (a review step when the reviewer approved); add a step only for work found later;
  `delivery_merge` merges the finished part while the steps ahead stay open. The delivery
  paragraph says the same of the merge. The briefing keeps its byte budget: the base input is
  not truncated and the tight budget (7,850 bytes) keeps every optional heading.
- PLN-007 (the tools' texts). `steps_set` (1-60 steps, all the known work, a step added later
  only for work found later), `step_update`, `spec_check` (at most 60) and `delivery_merge`
  (merges the finished part; refused while a step is in progress or none is finished) say the
  rule; so does the server's instruction line.
- PLN-008 (skills, workflows, templates). `delivery-pipeline` (sections 1 and 6),
  `om-spec-writing` (step 7), the Feature and Fix workflows' instructions, the spec template's
  Plan comment and the scaffolded AGENTS.md rule 3 say the same rule with the same numbers.
  One test reads the numbers in the shipped texts against the constants.
- PLN-009 (the plan's first size is kept). The store keeps, per task, how many steps the plan
  had when its first step was finished (`tasks.plan_first_steps`, migration `0056`): written
  once, in the transaction of the step write that finishes the first step, never changed
  after. A task whose steps were finished before the migration gets it at its next step write.
- PLN-010 (a plan that grew says so). `StepProgress.first_total` (API, absent when not known)
  carries it. Where the plan has more steps than it had then, the strip says "· N at first"
  after "Step n/m" (title: "The plan had N steps when its first step was finished; M were
  added since"), the task page's Plan tile says "d/t · N at first", and the tools say
  "(N at first)" after the count (`steps_get`, `steps_set`, `step_update`, `task_get`,
  `tasks_list`).
- PLN-011 (documents). SPEC-001 (STP-006, STP-008, GIT-001, DLV-008, AC-STP-4, AC-STP-6,
  §7.2), SPEC-002 (ADR-19's table, §5, §7.4), `docs/api.md`, `docs/product.md`, ADR-0325 with
  its index line and `docs/changes.md` state the rules as the code has them.

## Plan

- [x] The cause named with evidence from real tasks, and the spec
- [x] A pull request merges with later steps still ahead: the merge rule, `delivery_merge`, what the agent hears
- [x] A plan holds every slice: 60 steps, no "too fine" nudge, a nudge when it reads complete
- [x] The briefing and the tools' texts tell agents to plan all the known work
- [ ] Skills, workflows and templates say the same rule, checked against the constants
- [ ] The plan's first size is kept and shown where the plan grew: store, API, tools, strip
- [ ] SPEC-001, SPEC-002, the API reference, ADR-0325 and the changes log agree with the code
- [ ] Every gate, the e2e suite and the security scan run on the final code, with the evidence here
- [ ] Independent review's findings answered test-first

## Acceptance criteria

- AC-1 (PLN-001). `yulius_domain::delivery` `merge_gates`: a merge with two steps ahead and one
  finished is allowed; with steps and none finished it is `NoStepFinished`.
- AC-2 (PLN-002, PLN-003). `delivery::a_task_of_several_slices_merges_each_with_the_rest_ahead`
  (integration, the way #465 ran): a task with three slices as steps, the first finished.
  The end of a turn opens no pull request and says "still ahead … call `delivery_merge`".
  `delivery_merge` with the second in progress is refused with that step's title; with it
  pending the pull request opens and merges, the answer names 2 steps ahead and the next, the
  task reads 1/3 and is not Done. The second slice, committed and finished, goes out as a new
  pull request by `delivery_merge` with one step ahead.
- AC-3 (PLN-002). `delivery::a_merge_call_delivers_nothing_while_no_step_is_finished`: every step
  pending, `delivery_merge` is refused with "no step of the plan is finished".
- AC-4 (PLN-003). `agent_wait` `merged_text` with steps ahead says "2 steps of your plan are
  still ahead (next: "…")" and not "end with your summary"; without, the text is unchanged.
- AC-5 (PLN-004). `steps::plans_are_bounded`: 60 steps are accepted, 61 refused;
  `plan_shape_nudges`: 13 and 60 steps raise no nudge. `spec_format` `plan_items_are_bounded_and_unique`
  holds at the new bound.
- AC-6 (PLN-005). `steps::a_complete_plan_asks_for_the_work_still_known`: all steps done or
  skipped gives `Nudge::Complete`; one open step, or no steps, gives none.
- AC-7 (PLN-006). `briefing`: the needles "all the work you know of now", "at most 60" and
  "merges the finished part" are in every briefing (`assert_mandatory`), and the budget tests
  (`small_input_contains_everything_in_order`, `a_long_goal_shrinks_before_a_mandatory_part_is_cut`,
  `a_tight_budget_settles_in_a_few_passes`) pass.
- AC-8 (PLN-007, PLN-008). `skills::the_plan_rule_reads_the_same_everywhere`: the briefing's
  rule, the tool descriptions, the two skills, the two workflows and the scaffolded AGENTS.md
  each name `PER_PULL_REQUEST` and `MAX_STEPS` as the constants have them and say "all the work".
- AC-9 (PLN-009). `yulius-store` `the_plan_keeps_its_first_size`: a plan of 4 set with nothing
  finished has no first size; finishing a step (by `update_step`, and by `replace_steps`) sets
  4; growing to 11 and finishing more leaves 4.
- AC-10 (PLN-010). `mcp_text`: "Progress: 6/11 steps done (4 at first), 54% by effort." and
  "steps 6/11 (4 at first), 54%"; with a first size not below the total, no mention.
  e2e `steps.e2e.ts`: a task with 11 steps and 4 at first shows "· 4 at first" in the strip
  with its title; one that did not grow shows none.
- AC-11. A task run the way #465 was: its 18 slices as 18 steps from the start are accepted
  without a nudge, each slice's merge passes with the rest ahead, and the strip reads "Step
  6/18" where #465 read "Step 6/6" (AC-2 and AC-5 together).

## Test plan

- Domain, unit: `steps` (bounds, nudges, texts), `delivery` (`merge_gates`), `briefing`
  (needles, budgets), `spec_format` (bound, template comment, scaffold needle).
- App, unit: `mcp_text` (progress line, task line), `agent_wait` (`merged_text`), `skills`
  (the rule across shipped texts), `discipline` (default workflows still raise no nudge).
- Store: `the_plan_keeps_its_first_size`, and the migration list's position test.
- Integration (`crates/yulius/tests`): the two `delivery` tests of AC-2 and AC-3; the existing
  `a_ready_task_opens_its_pull_request_by_itself`, `an_undelivered_head_says_what_blocks_it`,
  `a_merge_call_opens_the_pull_request_a_goal_task_still_lacks` and the playground merge tests
  keep the turn-end rule; `mcp_coordination` for the tools' bound and texts.
- Web: `e2e/tests/steps.e2e.ts` for the strip, the whole e2e suite once.
- Failure cases: a step in progress at the call, no step finished, a first size of a plan that
  shrank (not shown), an older server without `first_total` (the strip shows nothing more).
- Each new test is run against the code without its change first, and the failure recorded in
  Evidence.

## Decisions

- **A pull request delivers the finished part of the plan; the steps are the whole task**
  (ADR-0325). Rejected: *steps with sub-steps or groups, one group per pull request* (a new
  column, a second level in every tool, text and view, and the merge rule would still have to
  learn which group a pull request is; nested Plan items already hold a slice's parts);
  *a mark on a step "for a later pull request"* (the same state said twice: a pending step is
  one nobody started); *chained tasks only* (the model the texts name, and #465 recorded why it
  did not fit: one ledger, one reviewer's context; the product already delivers a task's later
  work as new pull requests, ADR-0269).
- **Only the call is explicit, so only the call delivers a part.** The end of a turn keeps
  waiting for every step: an agent that ends a turn between two steps must not get a pull
  request and a CI run for half its work. `delivery_merge` names a reviewed head, which is the
  statement "this part is ready".
- **A step in progress holds the call, not the merge.** At the call it is the usual slip (the
  review step not ticked), and the refusal is the reminder the old rule gave. Once armed, the
  merge is not asked again about steps: the agent starts the next slice while the queue tests
  this one, and a refusal then would undo a reviewed, green merge.
- **The forgotten step is said, not refused.** A step left pending by mistake no longer stops
  the merge; the answer names the steps ahead, and the task stays "not Done" until they are
  resolved. The old refusal cost the same (the agent learned at the same call) and is what
  made agents keep later work out of the plan.
- **60 steps, no "too fine" nudge.** #465 names 20 slices and found three more while it ran;
  30 leaves little room for the next audit. A nudge that fires for every long task teaches
  agents to merge slices or to ignore nudges. Tiny steps are still said by the burst nudge.
- **A nudge when the plan reads complete, no hold on edits.** Holding a code edit while the
  plan reads n/n would make the rule certain, and would also stop a one-line fix of a failed
  check until a step exists for it; STP-008 keeps nudges from blocking. The nudge arrives with
  the tick that completes the plan, which is the moment the wrong 100 % would appear.
- **The first size is stored, not derived.** `step_events` has no record of a removed step, so
  the size of the plan at an earlier moment cannot be read back. "When the first step was
  finished" is the baseline because the spec's Plan has replaced the workflow's seed steps by
  then; a count taken at the first `steps_set` would be the seed plan's on nearly every task.
- **Assumed (reversible):** the Superagent, chains and the sidebar read the same counts and need
  no change of their own; the task page's Plan bar keeps its count ratio (it differs from the
  strip's effort share today, outside this change).
