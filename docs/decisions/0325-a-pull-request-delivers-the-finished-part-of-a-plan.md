# 0325. A pull request delivers the finished part of a plan

## Status

Accepted. Specified in [SPEC-484](../../.ai/specs/SPEC-484-task-plans-show-all-known-work-from-the.md);
task #466. Amends SPEC-001 STP-006, STP-008, GIT-001 and DLV-008 and SPEC-002 ADR-19 ("every
step is done" as a condition of the merge, the bound of 30 steps, the "too fine" nudge). Builds
on [ADR-0269](0269-work-after-a-tasks-merge-is-delivered-as-a-new-pull-request.md).

## Context

People, the Superagent and chains read a task's steps as how far the task is. On 2026-11-13
task #465 read "Step 6/6" with a full bar while its spec named 15 slices, 5 of them merged; it
had read 4/4, 2/2, 3/3, 4/5 and 5/6 before. The person: "You say 5 of 15 but I see only 6
steps. It is happening often that the plan isn't so credible and the number of steps grows in
the time."

The cause was a rule of the product. A merge was refused while any step was open, and so was
the push and the pull request before it: one pull request per task. #465 delivered one pull
request per slice. When it added the next slice's steps while a merge was armed, the answer was
"merge: 4 step(s) are still open", so it kept every later slice out of its plan and added each
one after the merge before it. Its plan could only ever read n/n before a merge.

The instructions pushed the same way. Every text asked for "3-12 steps", a plan of more than 12
was nudged to "merge related steps", a plan had at most 30 steps, and none said that the plan
holds all the known work. In the transcripts of 385 tasks the count of steps grew in 184; in 69
it grew right after the plan read n/n or (n-1)/n. Of 78 steps added at that point, 48 were a
review round added as its own step after the review step was ticked, and 16 a merge, a gate or
a later delivery.

## Decision

1. **The steps are the whole task; a pull request delivers the finished part.** A merge is no
   longer refused for steps that are not finished (`check_merge`). It is refused only for a
   plan none of whose steps is finished (`NoStepFinished`). Steps still ahead stay open; their
   work goes out as a later pull request of the same task (ADR-0269).
2. **Only an explicit merge delivers a part.** The agent's `delivery_merge` (it names a reviewed
   head) and a person's merge go out with steps ahead. A call that has a head to deliver first
   (a push, a pull request to open) is refused while a step is in progress, which is the work
   under way; a head the pull request already has is merged whatever step was started since.
   The delivery at the end of a turn still waits for every step, so a turn that ends between
   two steps opens no pull request.
3. **A merge is not the end of the task while steps are ahead, and their work waits for it.**
   Every message about a merge names the steps ahead and the next one instead of "end with your
   summary", and keeps what the delivery profile still asks for the merged part. The answers
   that arm a merge say that the next step's work starts once the merge is reported: a commit
   on the branch drops an armed merge of the reviewed head, as it always did. The task is not
   Done and the tasks chained after it wait, by the rules that already count open steps.
4. **A plan holds every slice.** 1 to 60 steps (`steps::MAX_STEPS`), 3-12 recommended for one
   pull request (`steps::PER_PULL_REQUEST`, in the texts only). The "too fine" nudge is gone.
   The briefing, the step tools, the `delivery-pipeline` and `om-spec-writing` skills, the
   Feature workflow and the templates ask for all the known work as steps from the moment it is
   known, a step added later only for work found later, and a step done once nothing of it
   remains (a review step when the reviewer approved). One test reads the numbers in the
   shipped texts against the constants.
5. **A complete plan asks once more.** When every step is finished the nudges hold "Every step
   is finished, so the task reads 100 %. If you know of work that is not done … add its steps
   now", with the tick that completes the plan.
6. **A plan that grew says so.** The store keeps, once, how many steps the plan had when its
   first step was finished (`tasks.plan_first_steps`). Where the plan has more now, the API
   carries `steps.first_total` and the strip, the task page and the tools say "N at first".

## Alternatives

- **Let the armed merge of the pushed head land while the branch is ahead of it.** The agent
  could then commit the next slice while the queue tests this one. The armed merge and the
  merge train both rest on "the reviewed head is the branch's head, with a clean worktree"; a
  change of that is its own slice. Until then the answers say to wait.
- **A playground delivers a part too.** A merged playground task delivers nothing again (there
  is no follow-up squash as there is a follow-up pull request, ADR-0269), so a part squashed
  first would leave the rest undeliverable. A playground keeps "one squash, the whole plan" and
  says so in its refusal.
- **Steps with sub-steps, or groups of steps, one group per pull request.** A long task would
  show slices with their parts. It needs a second level in the store, the API, every tool and
  text and the strip, and the merge rule would still have to learn which group a pull request
  is. Nested items of the spec's Plan already hold a slice's parts without being steps.
- **A mark on a step, "for a later pull request".** It says twice what the status says: a
  pending step is one nobody started. The agent would have to keep the mark right as slices
  move, and a forgotten mark would bring the old refusal back.
- **Chained tasks only.** It is the model the texts named for larger work, and it stays. #465
  recorded why it did not fit an audit: one ledger of findings and one reviewer's context, and
  a slice's fix changes how the next is best made. The product already delivers a task's later
  work as new pull requests.
- **Keep the refusal for a pending step and add a parameter "this is a part".** The refusal
  reminded an agent of a step it forgot. The reminder costs the same as an answer that names
  the steps ahead (the agent learns at the same call), and an armed merge would have to
  remember the parameter.
- **Hold code edits while the plan reads n/n.** It would make "never 100 % with work ahead"
  certain. It would also stop a one-line fix of a failed check until a step exists for it, and
  nudges do not block (STP-008). Kept as the next measure if the nudge proves too weak.
- **Derive a plan's growth from `step_events`.** The history has no record of a removed step,
  so the size of the plan at an earlier moment cannot be read back.

## Consequences

- A task run the way #465 was lists its 18 slices as 18 steps from the start; each slice's
  merge passes with the rest ahead, and the strip reads "Step 6/18" where it read "Step 6/6".
- A step left pending by mistake no longer stops a merge. The answer names it, the task stays
  out of Done, and its agent is told to go on or to resolve it.
- A person's merge through the API merges the head the pull request has, with steps ahead and
  whatever step is in progress; it is refused only while no step is finished.
- A turn's end with a pull request open and steps ahead reads GitHub once, to tell a delivered
  head from one that is not.
- "Open steps: N" in `delivery_status` and the Git page is the work ahead, not a reason the
  merge is refused.
- Tasks whose steps were finished before schema version 56 get their plan's first size at
  their next step write: the plan as it is then.
- The plan's first size is taken when the first step is finished. An agent that finishes a
  step of its workflow's seed plan before it writes its own plan gets the seed plan's size as
  "at first".
