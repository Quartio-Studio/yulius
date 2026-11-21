---
yulius-task: b2d041b9-f438-4630-8b8c-a686f9c0ef0d
status: in-progress
---

# SPEC-589 Continue authorized work at the end of a goal turn

## Goal

A goal task whose next action is already authorized continues that work itself. Its owner can leave the task running through verification on dev.

Fixes #1600.

## Context

The screenshot shows a live goal task with one open step, “Verify the merged revision on dev”. The system asks the owner to type “continue”. It does not identify the harness or sandbox. The shared Stop hook validates the spec and occasionally nudges for stale or untracked progress. An ordinary in-progress step meets neither condition. Settling the turn then derives a Needs You notice from the open plan. The Antigravity idle Stop regression even expects this result.

Earlier user requests establish the ownership rule. Task #534 concerned automated CI waits ([SPEC-552](SPEC-552-on-codex-agent-keeps-calling-me-even-if.md)), #540 asked for bounded continuation of unfinished Codex goals ([SPEC-558](SPEC-558-parked-codex-tasks-no-codex-exit-screen.md)), and #572 removed requests to authorize another independent review ([SPEC-588](SPEC-588-it-shouldn-t-ask-me-for-this-agents-must.md)). Post-merge machine evidence is already available through [SPEC-562](SPEC-562-complete-post-merge-verification-from-du.md).

## Requirements

- GCT-001. At a clean Stop of a live goal-mode task, continue actionable plan work or an unfinished spec under the existing authorization. Apply the rule across harnesses, including when the remaining step is post-merge verification. Preserve prompt-mode turn boundaries and the native goal evaluator.
- GCT-002. An outstanding human decision, a pause, an active background operation, or delivery news that already owns the next turn takes precedence over a continuation for declared work. The existing spec-format gate still checks required artifacts. A failed Antigravity turn uses its existing error path. Completion requires the normal evidence and step updates.
- GCT-003. Use the existing persisted Stop retry limit. When continuations are exhausted, expose the unsuccessful attempts and remaining work. A completed step restores the attempt budget. A subscription move or a capacity wait preserves it. A plan whose remaining steps are explicitly blocked shows the recorded dependency. A clean stop with finished work releases the turn.

## Plan

- [x] Trace the reported handoff against earlier continuation requests
- [x] Reproduce the incorrect handoff in regression tests
- [x] Route unfinished work to its automatic continuation owner
- [x] Preserve goal continuation through capacity recovery and a busy browser
- [x] Track completed background work and concurrent plan progress at Stop
- [x] Preserve the in-flight turn during spec publication and a requested subscription switch
- [x] Distinguish owner input from native goal restoration and preserve prompt-mode rotation
- [ ] Keep a submitted owner instruction through concurrent typing and capacity holds
- [ ] Verify the fix and obtain independent approval

## Acceptance criteria

- AC-1. The Stop response for a goal with the screenshot's one remaining step instructs the agent to continue. Task detail shows Working, preserves the step and records no Needs You episode.
- AC-2. A clean Stop can end a finished goal turn or a prompt-mode turn. A real owner question or a machine wait retains its owner.
- AC-3. An unfinished spec can continue even after the plan is complete. Repeated stops without completing the work reach the existing three-block limit.

## Test plan

Exercise the real HTTP hook and task query using isolated SQLite and Git fixtures. Check each harness, a verification-only step, spec-only work, completed work, prompt mode, existing human and machine handoffs, and repeated stops. Run the affected Rust tests and repository checks. CI verifies the full change.

## Decisions

[A goal turn continues its declared work](../../docs/decisions/0428-a-goal-turn-continues-its-declared-work.md).

Make the continuation decision where the agent can act on it, at the shared Stop hook. Sidebar and notification consumers can continue to report the task's actual state. Use the existing hook retry limit, delivery wake and human-request records. The earlier Codex prototype in #540 informed the ownership checks. Only a confirmed background operation or pending delivery wake takes ownership. A missing process observation permits the bounded continuation in the existing session. Rotation retains its separate evidence requirement before moving that session. PR-only recovery and parked-session repair have their own delivery paths.

## Evidence

- Independent review at `6ed3c8612` requested changes. Eight regressions failed on that implementation. A capacity release published Waiting, a busy browser exceeded the hook deadline, and a Stop erased a worker permission. A blocked step consumed an attempt. Finishing a step left the previous retry budget exhausted. A proactive move hid the exhaustion explanation, reset the durable counter and allowed its launch prompt to reset it again.
- Review finding 1. The turn hold now carries its continuation. An in-place release publishes Working, and a process-free restart reconstructs the current next action even for older holds without text. The subscription and subprocess fixtures exercise each recovery path.
- Review finding 2. Live browser observations during Stop share a deadline, including task-view reads during spec and step synchronization. Updating only the internal retry count needs no publication. The HTTP fixture holds each lookup in turn.
- Review finding 3. An unfinished goal's clean Stop preserves an outstanding permission and its original question until the answer arrives. The worker permission fixture checks the durable activity and question text.
- Review finding 4. Only pending and in-progress steps receive a continuation. A plan containing only blocked work shows the recorded dependency and spends no attempt. The blocked-step fixture checks both.
- Review finding 5. Exhaustion is explained before proactive rotation. A carried continuation spends an attempt when admitted for launch, and its own submit hook preserves the count across the move. Completing a previously open step resets the count in the same transaction that records progress. The tests exercise an actual subprocess move, its prompt, exhaustion and both plan update APIs.
- The eight review regressions passed with the eight original continuation controls. A process-free legacy hold also recovered its verification instruction in a real subprocess. Two further assertions failed on the integrated head: a stale Waiting row still showed the generic notice after a block, and an exhausted goal used the prompt-mode instruction to tell the agent how to go on. The continuation now conditionally restores Working and the exhaustion notice names the attempts and points to their record.
- On dev revision `86eb47202`, `cargo test -p yulius --test main agent_hooks::goal_stop_` returned three failures. The post-merge case reproduced the screenshot's notice and its wait-history entry. The unfinished-spec case and the case where a machine wait had ended both returned an empty Stop answer. The explicit permission request and held scope decision retained their owner.
- On the first implementation the verification case passed for all three harnesses. The unavailable-host regression returned an empty Stop answer. It exercises a live attempt with a recorded child PID and a host that cannot read the process tree.
- The second independent review at `c6d91072d` found four regressions. All four new HTTP tests failed against that head. Antigravity's idle Stop inherited a stale worker flag. A Stop used a completed step and its old, exhausted budget after a worker advanced the plan during browser observation. Spec synchronization published a generic Needs You episode before restoring Working. A continuing goal also missed its one-time instruction to update a stale step.
- Second review findings 1 and 2. Antigravity's explicit fully idle report clears its previous background observation. After observing live ownership and rotation, Stop reads the current plan and retry count under the same lock used by plan writers, then makes its continuation decision. The tests use consecutive real hooks and a controlled browser observation with a concurrent worker update.
- Second review findings 3 and 4. Task and step updates generated by the goal's Stop are published together after its final activity is known. The fixture checks the UI event stream and durable owner wait history. A stale or untracked goal step receives its one-time tracking guidance alongside the continuation.
- The 24 scoped continuation and recovery tests passed at `5b824a5e8`, including queued starts for new and existing provider sessions. A third independent review identified a spec watcher that could retain the plan lock during publication and an already-working turn whose public subscription switch reset its budget.
- Third review finding 1. A separate spec watcher blocked in its browser lookup kept the goal Stop from answering within four seconds on `5b824a5e8`. The watcher now publishes after releasing the plan lock. Its regression requires Stop to name the newly recorded step while the watcher is still held.
- Third review finding 2. The public Switch action's real replacement subprocess received the normal interrupted-turn prompt. Its submit hook reset two consumed attempts to zero on `5b824a5e8`. Recognizing that instruction preserved the two attempts through the opening turn. The fourth review followed the replacement process through native goal restoration as well.
- The first frozen review-fix mutation run reported `76 listed, 27 caught, 8 missed, 0 timed out, 38 unviable, 3 not reached` at its 30-minute bound. The eight survivors exposed missing assertions about session lineage, in-place release activity, fresh owner submissions and browser tab ownership. The new fixtures check each state, including a live browser action on another task. The 14 Chromium tests for attention and owner wait history passed.
- The fourth review at `a46889ab8` showed that restoring a native goal reset the count after the opening turn. A text comparison also classified identical words from the owner as an automatic resume. The public switch test now observes the actual restored goal, submits its generated command and submits the same resume text as a person. Trusted owner messages delivered through the task tool also restore the budget. Four controls failed before the fix, covering native restoration, tool delivery, machine waits and prompt-mode rotation at exhaustion.
- The shared hook now identifies a fresh owner submission from recorded terminal input. Tool delivery uses its authenticated sender and successful send. Automatic launches, restored goals and background handoffs retain the existing attempts. Prompt-mode rotation still runs at its original place in the spec gate.
- A frozen whole-change mutation run reported `104 listed, 23 caught, 6 missed, 0 timed out, 18 unviable, 57 not reached` at its 30-minute bound. The six surviving mutations concerned capacity release, input attribution and the deadline for a live browser action. The recovery, owner-input and live-tab tests now check those outcomes. The spec publication test also checks its StepsUpdated event. An earlier follow-up mutation run stopped at its baseline because the browser fixture lacked usable capacity. The live-tab test now provisions capacity and observes the browser request in flight.
- At `82d06c58c`, all 29 focused Stop, owner-input and recovery controls passed. Clippy passed for the app, store and executable with all targets, all features and warnings denied. The next frozen follow-up mutation run reported `64 listed, 13 caught, 1 missed, 0 timed out, 19 unviable, 31 not reached` at its 30-minute bound. The surviving removal of step publication is covered by the StepsUpdated assertion.
