---
yulius-task: 3936a7cc-f429-472d-8370-43b2e879bf90
status: done
---

# SPEC-484: a task's steps show all the work it knows of

Status: done
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
- **How common, from the transcripts of 385 tasks** (what `steps_set`, `step_update` and
  `steps_get` answered, in time order; `target/evidence/step-history.txt`): the count of steps
  grew in 184 tasks (48 %), by three or more in 65, to at least double in 31. In 69 tasks (18 %)
  it grew right after the plan read n/n or (n-1)/n, in 34 of them from n/n. Which steps were
  added (the plans `steps_set` and `steps_get` answered with, 179 tasks,
  `target/evidence/step-additions.txt`): 613 steps, 410 of them before any step was finished (a
  workflow's seed plan replaced by the task's own: harmless). Of the 203 added after that (78
  tasks), 140 name a review round or its findings, 29 a merge, a gate or a delivery, 34 other
  work. 78 were added while the plan read n/n or (n-1)/n (34 tasks): 48 a review round, 16 a
  merge, gate or delivery ("Second delivery: …", "After the merge: …"), 14 other work.
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
  (`DeliveryError::NoStepFinished`: "no step of the plan is finished: mark done what this
  delivery includes (`step_update`)", one wording with the call's, `steps::NOTHING_FINISHED`).
  A plan without steps is as before. `DeliveryFacts` carries `steps_open` and `steps_finished`.
  A playground is the exception: it squashes a task once, with its whole plan (a merged
  playground task delivers nothing again, so a part squashed first would leave the rest
  undeliverable), and refuses a squash with steps open: "N step(s) of the plan are still open
  (next: "…"): a playground squashes a task once, with its whole plan; finish them, or resolve
  them with `step_update` and a note".
- PLN-002 (what `delivery_merge` waits for). The delivery a `delivery_merge` call makes (push,
  pull request, a new pull request after a merge) goes out with steps still ahead. A call that
  has a head to deliver is refused while a step is in progress ("step "…" is in progress:
  finish it if this delivery includes it, or set it back to pending (`step_update`) when it
  belongs to a later one") and while no step is finished. A head the open pull request already
  has is merged whatever step was started since, by the agent's call and by a person's merge
  alike, and the delivery view offers that merge. The delivery at the end of a turn stays as
  it is: it waits for every step, so a turn that ends between two steps opens no pull request.
  Its reason for a plan with finished steps and others ahead names the way: "N step(s) are
  still ahead (next: "…"): Yulius delivers by itself once every step is finished; to deliver
  the finished part now, call `delivery_merge` with its reviewed head". A turn that ends on a
  head its open pull request already has leaves no such note; a commit above an open pull
  request is said to be "not pushed to its open pull request".
- PLN-003 (the agent hears what is ahead). The answers of `delivery_merge` (merged, armed,
  armed already, left for the next pass) and every message about a merge on GitHub
  (`merged_text`, the armed merge's, the merge train's) name the steps still ahead and the
  next one. Where they said "end with your summary" they say "go on with the next step"; what
  the delivery profile still asks for the merged part stays in them. The answers that arm a
  merge say that the work of the steps ahead starts once `yulius wait` reports the merge: a
  commit on the branch drops an armed merge of the reviewed head, as it always did
  (`merge_armed_task`). With none ahead the messages are as before. A merged task with steps
  ahead is not Done and the tasks chained after it wait (existing rules, pinned here by a
  test).
- PLN-004 (a plan holds every slice). A plan has 1 to 60 steps (`steps::MAX_STEPS`; the spec's
  Plan, `steps_set` and a workflow's seed steps share the bound, and a scope question lists a
  whole plan, `intake::STATE_MAX_STEPS`). The "too fine" nudge (`Nudge::FinePlan`) is removed:
  more than 12 steps is what a task of several pull requests has. `steps::PER_PULL_REQUEST`
  (12) is the recommended most for one pull request, in the texts only.
- PLN-005 (a complete plan asks once more). When every step is finished, the nudges hold
  `Nudge::Complete`: "Every step is finished (N of N), so the task reads 100 %. If you know of
  work that is not done (a later slice or pull request, the review of a fix), add its steps
  now (`steps_set`)." It comes with the step tools' results and the briefing like every nudge.
  In the briefing the nudges stand before the list of steps: at the default budget a long
  plan's list does not fit whole and is what gets cut, so its nudges stay (`steps_get` has all
  of the list). With a workflow's instructions taking their room (the tight budget of the
  briefing's tests, 7,850 bytes) the steps part can be cut down to its heading, nudges
  included: it is an optional part of the briefing, as before.
- PLN-006 (the briefing). Rule 3 says: plan all the work known now, later slices and pull
  requests included, 3-12 steps per pull request and at most 60; a step is done when nothing of
  it remains (a review step when the reviewer approved); add a step only for work found later.
  Two of its sentences gave way for the room ("Declare `effort` 2 or 3 only for a step that
  cannot be split; progress is weighted by it", which `steps_set` says, and "A finished step
  stays finished; new work gets a new step", which `step_update` says and "add a step only for
  work found later" replaces). The delivery paragraph says that Yulius delivers at a turn's
  end once all steps are done, or when `delivery_merge` is called for the finished part. A
  playground's briefing keeps its sentence. The briefing keeps its byte budget: the base input
  is not truncated and the tight budget (7,850 bytes) keeps every optional heading.
- PLN-007 (the tools' texts). `steps_set` (1-60 steps, all the known work, a step added later
  only for work found later), `step_update`, `spec_check` (at most 60) and `delivery_merge`
  (merges the finished part; refused while none is finished, and while a step is in progress
  when the call has a new head to deliver; the work ahead begins after the merge; a playground
  squashes once) say the rule; so does the server's instruction line.
- PLN-008 (skills, workflows, templates). `delivery-pipeline` (sections 1, 5 and 6; section 6
  also says that the next step's work begins after the merge and that a playground delivers
  once),
  `om-spec-writing` (step 7), the Feature workflow's instructions, the spec template's Plan
  comment and the scaffolded AGENTS.md rule 3 say the same rule with the same numbers. (The Fix
  workflow states no count and its instructions are at their byte limit: unchanged.) One test
  reads the numbers in the shipped texts against the constants.
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
- PLN-011 (documents). SPEC-001 (STP-006, STP-008, the new STP-009 with AC-STP-7, GIT-001,
  DLV-008, AC-STP-4, AC-STP-6, §7.2), SPEC-002 (ADR-19's table, §5, §7.4, §7.6), `docs/api.md`,
  `docs/product.md`, `docs/engineering/workflow.md`, `e2e/README.md`, ADR-0325 with its index
  line and `docs/changes.md` state the rules as the code has them.

## Plan

- [x] The cause named with evidence from real tasks, and the spec
- [x] A pull request merges with later steps still ahead: the merge rule, `delivery_merge`, what the agent hears
- [x] A plan holds every slice: 60 steps, no "too fine" nudge, a nudge when it reads complete
- [x] The briefing and the tools' texts tell agents to plan all the known work
- [x] Skills, workflows and templates say the same rule, checked against the constants
- [x] The plan's first size is kept and shown where the plan grew: store, API, tools, strip
- [x] SPEC-001, SPEC-002, the API reference, ADR-0325 and the changes log agree with the code
- [x] Every gate, the e2e suite and the security scan run on the final code, with the evidence here
- [x] Independent review's findings answered test-first

## Acceptance criteria

- AC-1 (PLN-001). `yulius_domain::delivery` `merge_gates`: a merge with nine steps ahead and
  one finished is allowed; with steps and none finished it is `NoStepFinished`.
  `playground::an_unfinished_task_is_not_merged` and
  `playground::a_playground_squashes_a_task_once_with_its_whole_plan`: a playground refuses a
  squash with a step ahead, for a person and for its agent, and squashes the finished plan.
- AC-2 (PLN-002, PLN-003). `delivery::a_task_of_several_slices_merges_each_with_the_rest_ahead`
  (integration, the way #465 ran): a task with three slices as steps, the first finished, and
  a task chained after it. The end of a turn opens no pull request and says "still ahead …
  call `delivery_merge`". `delivery_merge` with the second in progress is refused with that
  step's title; with it pending the pull request opens and merges, the answer names 2 steps
  ahead and the next, the task reads 1/3 and the chained task stays chained. The second slice,
  committed and finished, goes out as a new pull request by `delivery_merge` with one step
  ahead; the armed answer, and the answer to a second call, say that the step ahead waits for
  the merge. The turn that ends on it leaves no "not delivered" note. A commit made all the
  same is "not pushed to its open pull request" and drops the armed merge. With the commit
  taken back and the step ahead only marked in progress, the call arms the head the pull
  request already has and the armed merge lands; its message names the step ahead instead of
  the task's end; the chained task still waits, and is released once every step is finished.
  `delivery::a_head_the_pull_request_already_has_merges_whatever_step_was_started_since`: the
  same for a person's merge, and the delivery view offers the merge with nothing blocked.
  `delivery::a_merge_left_for_the_next_pass_says_the_step_ahead_waits` and
  `merge_train::armed_pull_requests_merge_together_on_one_run`: the answer of a merge left for
  the next pass and the merge train's message.
- AC-3 (PLN-002). `delivery::a_merge_call_delivers_nothing_while_no_step_is_finished`: every step
  pending, `delivery_merge` is refused with "no step of the plan is finished".
- AC-4 (PLN-003). `agent_wait` `a_merge_with_steps_ahead_says_the_task_goes_on`: `merged_text`
  with steps ahead says "2 step(s) of your plan are still ahead (next: "…")" and "the next
  step", not "end with your summary", and keeps what the profile still asks; without, the
  text is unchanged.
- AC-5 (PLN-004). `steps::plans_are_bounded`: 60 steps are accepted, 61 refused;
  `plan_shape_nudges`: 13 and 60 steps raise no nudge. `spec_format` `plan_items_are_bounded_and_unique`
  holds at the new bound.
- AC-6 (PLN-005). `steps::a_complete_plan_asks_for_the_work_still_known`: all steps done or
  skipped gives `Nudge::Complete`; one open step, or no steps, gives none.
  `briefing::steps_show_weighted_progress_and_nudges`: the nudges stand before the list, and a
  plan of 60 finished steps, whose list is cut, still says that it reads complete.
- AC-7 (PLN-006). `briefing`: the needles "plan all the work you know of now, later slices and
  pull requests too", "(3-12 per pull request, at most 60)", "done once nothing of it remains
  (a review: once approved)", "Add a step only for work found later" and "or when you call
  `delivery_merge` for the finished part (steps ahead stay open)" are in every briefing of a
  GitHub sandbox (`assert_mandatory`), and the budget tests
  (`small_input_contains_everything_in_order`, `a_long_goal_shrinks_before_a_mandatory_part_is_cut`,
  `a_tight_budget_settles_in_a_few_passes`) pass.
- AC-8 (PLN-007, PLN-008). `skills::the_plan_rule_reads_the_same_everywhere`: the two skills,
  the Feature workflow, the scaffolded AGENTS.md and `steps_set` each say "all the work", name
  `COARSE_BELOW`-`PER_PULL_REQUEST` and `MAX_STEPS` as the constants have them, say that a step
  is added only for work found later, and none says "3-10", "1-30" or "at most 30";
  `delivery_merge` and the skill's merge rule say "the finished part" and not "every step is
  done"; a new spec's Plan comment asks for all the work known now.
- AC-9 (PLN-009). `yulius-store` `the_plan_keeps_its_first_size`: a plan of 4 set with nothing
  finished has no first size; finishing a step (by `update_step`, and by `replace_steps`) sets
  4; growing to 11 and finishing more leaves 4.
- AC-10 (PLN-010). `mcp_text`: "Progress: 6/11 steps done (4 at first), 54% by effort." and
  "steps 6/11 (4 at first), 54%"; with a first size not below the total, no mention.
  e2e `steps.e2e.ts`: a task with 11 steps and 4 at first shows "· 4 at first" in the strip
  with its title; one that did not grow shows none.
- AC-11. A task run the way #465 was: its slices as steps from the start are accepted without a
  nudge (18 through `steps_set`, `mcp_coordination::a_long_plan_is_not_nudged_and_a_complete_one_asks_once_more`;
  36 through the spec's Plan with their parts nested,
  `agent_hooks::a_plan_of_every_slice_becomes_steps_and_is_not_told_to_shrink`), each slice's
  merge passes with the rest ahead (AC-2), and the strip reads "Step 6/18" where #465 read
  "Step 6/6".

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
- **A step in progress holds the delivery of a head, not the merge of a delivered one.** At a
  call that has a head to push it is the usual slip (the review step not ticked), and the
  refusal is the reminder the old rule gave. A head the pull request already has holds nothing
  of a step started since, so neither the agent's call, nor a person's merge, nor an armed
  merge is refused for it (the spec sync and the step tools may change a status at any time).
- **The work of the steps ahead waits for the merge.** A commit on the branch drops an armed
  merge of the reviewed head, and the merge train needs a clean worktree at that head: older
  rules, kept. This change's first draft claimed the agent could start the next slice while
  the queue tests this one; the review showed that only a changed status survives, not a
  commit, so the armed answers say to wait. Letting the pushed head merge while the branch is
  ahead of it is the larger change (ADR-0325, alternatives).
- **A playground squashes once, with its whole plan.** It has no follow-up delivery (ADR-0269
  is GitHub's), so a part squashed first left the rest of the task undeliverable: found by the
  review with a probe, and decided for the rule that exists rather than a second delivery
  mechanism in this change.
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
- **One pull request, about 470 lines of code outside tests.** The parts depend on each other
  (the texts may only ask for every slice as a step once a merge passes with steps ahead), and
  the server this task runs on still refuses a merge while a step is open, so this task could
  not itself deliver in two pull requests with the second one's steps in its plan.
- **The holds say "this delivery", not "this pull request".** The same sentences serve the
  call and the merge rule, and the delivery may be a follow-up pull request.
- **Assumed (reversible):** the Superagent, chains and the sidebar read the same counts and need
  no change of their own; the task page's Plan bar keeps its count ratio (it differs from the
  strip's effort share today, outside this change).

## Evidence

Commands run in this task's worktree, at the head named. Logs are kept in the ignored
`target/evidence/`; the reviewer's notes and probes in `$HOME/.cache/yulius-task-466/`.

- **The cause, from real tasks.** `target/evidence/task-465-steps.txt` (the plans #465 set and
  the refusal it got: "2026-11-13T02:19 delivery_status: Open steps: 4 … merge: 4 step(s) are
  still open", then the plan cut back to finished slices only), `step-history.txt` (385 tasks:
  the count grew in 184, in 69 right after n/n or (n-1)/n) and `step-additions.txt` (613 added
  steps by kind). The numbers in Context are these files'; the reviewer recounted them.
- **Each test fails without its change.** `python3 target/evidence/mutants.py` undoes one
  change at a time on a clean tree and runs the tests that pin it: 28 mutants at 207e6ebd8,
  "every mutant was killed" (`mutants.log`). They cover the merge rule (both ways), the call's
  two holds, the turn's end, a head the pull request already has, the wording above an open
  pull request, the playground's whole-plan squash, the three merged messages and the two
  kinds of answer, the bound, the complete nudge, the briefing's and the tools' texts, the
  skill and workflow texts, the four store rules of the first size, and its way through the
  API and the tools. Not mutated: the order of nudges and list in the briefing
  (`briefing::steps_show_weighted_progress_and_nudges` asserts the order and a cut list that
  keeps its nudge). Two web mutants by hand (`mutant-web-1.log`: a first size not below the
  plan's is shown; `mutant-web-2.log`: the strip shows none): `steps.e2e.ts` fails for each.
- **A defect of this change, found by reading it and fixed test-first.** With a part delivered
  and armed and steps ahead, the end of the turn noted "Yulius has not delivered …" for the
  head the open pull request already had, which ends the agent's `yulius wait` at once. The
  test reproduced it (`push_problem` set); `try_auto_open_pr` now asks about the plan after the
  open pull request's head. Commit 13325719b.
- **The independent review, round 1** (a fresh session, read-only, `om-code-review`, head
  2c4a4318c): "**request changes.** There are three major findings in the change's own code".
  Answered in 828dcf88a, each test-first:
  - M1, "A playground task that delivers a part can never deliver the rest": true (its probe 4).
    Of its two corrections the second is taken: a playground squashes a task once, with its
    whole plan, and says so. `playground::a_playground_squashes_a_task_once_with_its_whole_plan`,
    `playground::an_unfinished_task_is_not_merged`.
  - M2, "A commit for the next step drops the armed merge, and the test hides it": true (its
    probe 1). The dropping rule is older and stays; the armed answers now say that the work of
    the steps ahead starts once `yulius wait` reports the merge, the claim is corrected here,
    in ADR-0325 and in SPEC-001, and the test keeps the commit and asserts "The merge you armed
    was dropped".
  - M3, "A new test cannot fail for this change": true. The test now has 36 slices (30 was the
    bound) and reads the nudges where the briefing has them; it fails for the mutant "a plan
    has at most 30 steps".
  - m1, "'Refused while a step is in progress' is three different rules": one rule now, the
    hold belongs to delivering a head; a person's merge has no check of its own.
    `delivery::a_head_the_pull_request_already_has_merges_whatever_step_was_started_since`
    (its probes 2 and 3 as a test).
  - m2, "The delivery view says Merge is ready when the call is refused": with m1 the view and
    the call agree for a head the pull request has (asserted in the same test).
  - m3, the playground's "merged meanwhile" message: a playground has no step ahead at a merge
    any more (M1), so the message is true as it is.
  - m4, "PLN-003 call sites without tests": the merge train's message
    (`merge_train::armed_pull_requests_merge_together_on_one_run`, one task with a slice
    ahead), the "armed already" answer and the armed merge's message (the AC-2 test), the
    answer of a merge left for the next pass
    (`delivery::a_merge_left_for_the_next_pass_says_the_step_ahead_waits`), and the chained
    task that waits until every step is finished (the AC-2 test, with the release as control).
  - m5, "Every turn's end now reads GitHub while a pull request is open with steps ahead":
    kept, and said (the comment in `try_auto_open_pr`, ADR-0325's consequences): the delivery
    record has no head to compare with, and that state did not exist before.
  - m6, "The briefing cannot show a long plan or its nudges": the nudges stand before the list
    now, so a cut list keeps them; that the list itself is cut is said in PLN-005.
  - m7, m8 and the nits: `docs/api.md`, the comments, the quotes in AC-4 and PLN-005, the
    briefing's delivery sentence ("at your turn's end once all steps are done, or when you
    call …"), one wording for "nothing finished" (`steps::NOTHING_FINISHED`), the Deliver
    dialog's line about steps ahead (no longer a warning), the redundant alternative in the
    skills test, `STATE_MAX_STEPS` and the two sentences rule 3 lost (now in PLN-004 and
    PLN-006), "(not pushed to its open pull request)", and when a new pull request opens for
    work after a merge.
- **The independent review, round 2** (the same reviewer, head 5a7ed78ef): "**approve.** Every
  round-1 finding is answered, and each of the three majors now has a test built to fail
  without its fix. The delta brings no new blocker, major or minor: five nits, the author's
  call." Its own gate and e2e suite passed at that head. The nits, answered in 207e6ebd8:
  - n1 (at the tight budget the nudges are cut too): PLN-005 now says at which budget the
    order helps; the briefing's code is left as it is.
  - n2 (a playground assertion that could not fail): removed, with the claim in the test's
    comment.
  - n3 (the assertion that `merged_text` without a next step names no steps ahead): back.
  - n4 (`delivery-pipeline`, section 6): it says that the next step's work begins once the pull
    request is merged, and that a playground delivers a task once.
  - n5 (pre-existing: a playground task's delivery view lists `open_pr` and `promote`): read
    in the code (`candidates` knows no playground) and filed as issue #1258; outside this
    change, which touches neither `candidates` nor the view's next actions.
- **The gate.** `scripts/dev ci` at 207e6ebd8, the last head with code: exit 0 (`gate-3.log`,
  "4225 tests run: 4225 passed, 16 skipped", clippy, the macOS build, doctests, `cargo deny`,
  `cargo shear`, `cargo doc`). Earlier runs: at 83944755d before the review, exit 0
  (`gate-1.log`); at 39db6b6c2 one test of 4220 failed (`gate-2.log`):
  `task_done::a_stop_settles_a_run_no_server_holds`, "the run's host still runs", the open
  issue #1190 ("fails … under full-gate load"; the load average was 74), in code this change
  does not touch; alone it passed three times in a row, and in the two later runs of the gate
  (the reviewer's at 5a7ed78ef, this one).
- **The e2e suite.** `cd e2e && npx e2e run` at 39db6b6c2: 1021 passed, 1 skipped, exit 0
  (`e2e-2.log`); the reviewer's run at 5a7ed78ef: the same. Nothing under
  `crates/yulius-server/web` or `e2e/` changed after it. An earlier run at 83944755d had one
  failure, `tests/cd.e2e.ts` "a needs_you decision reads Decide, and an action alone only To
  do" (open issue #1171, load), which passed alone (26 of 26).
- **The strip, seen.** `target/evidence/strip.png` (mock `planfirst=4`, 1280 px): "#12 Checkout
  API  Step 3/7 · 4 at first · Implement checkout API". At 760 px the count gives way whole,
  as before.
- **The security scan.** `yulius security-scan`: "Security scan disabled: Vidoc is turned off
  in Yulius, for this project or the whole instance; nothing was scanned." Disabled, not passed.
- **`dev` merged.** #465's branch, which this task started from, was squash-merged with the
  same tree (`git diff --quiet 67ab28d09 1afaa65af`: exit 0), taken with `-s ours`; #465's
  slice 12 (1206541e1) merged for real: one conflict, the migration list (dev's 0055 is schema
  version 55, this task's 0056 is 56), and the decision index repaired after the union merge
  repeated ADR-0323's line; #465's slice 13 (3e3b09e13) merged without a conflict, and clippy
  and the tests of what it touched (the merge train, CI, playgrounds: 285) passed on the result.
- **The head with this Evidence** differs from 207e6ebd8 by this document only (status, the
  last Plan item, this section). The reviewer's look at it is named in the merge's review
  summary, since its verdict cannot be in the commit it judges.
- **Not tested, and why.** The running server (397d5bdd, the instance's own build) has the
  old rules, so this task's own delivery cannot show a merge with steps ahead: its steps are
  all finished before its merge. A live session's reading of the new briefing and tool texts
  starts when the instance is updated to a build with this change (a person's promotion).
