---
yulius-task: ad87074f-2de4-4f60-989b-e5d2217b45e1
status: done
---

# SPEC-631 A merged task whose own wait outlives its staging outcome ends Done

## Goal

A merged task whose staging request ends (verified, gate_failed or deploy_failed) either gets the
outcome typed into its session or shows Done when nothing is left for it, without a person
closing it. CD runs started for a task's staging request say which task asked.

## Context

Incident, quartio-v2 task #377 (id 86103dc0-be80-44f2-a579-9ca6f6d1fea9, Claude Code, Opus 5.5,
workflow "Fix"), 2026-11-23 UTC: PR #1623 merged into dev at ec4e7b48 around 11:05 with all five
plan steps checked and the spec done. At 11:06 the task called `cd_deploy dev` (staging request
8d7d15c1, testing). At 11:12:36 it ended its turn with one background shell still running; its
`state_since` and `updated_at` never changed after that. Candidate CI run 3b9642c5 passed at
11:22:52, CD run 982a24b6 deployed and verified dev, and `task_get` showed "Staging verified".
Minutes later the task still read "running (waiting)", nothing had been typed into its session,
and the owner closed it at about 11:30.

### Root cause

Two rules contradicted each other for one task:

1. **The outcome was recorded silently.** `App::tell_staging_outcomes`
   (`crates/yulius-app/src/ci/staging.rs`) types an outcome only into a task that still accepts
   machine notices (`machine_notice_tasks`: open steps, an open pull request, or unmerged work).
   #377 had merged, its plan was complete and its spec said done, so it did not: the outcome was
   marked told without being typed (SPEC-612 GF-006, ADR-0450: a finished task gets no machine
   prompt). The session-input boundary (`type_into_session_written`) refuses Yulius's notices to
   such a task for the same reason.
2. **The task could not be Done.** Its idle agent had left a background shell running. A running
   background shell keeps an idle task Working (`AttentionFacts::group`, `App::task_is_done`),
   because Claude Code wakes the agent when the shell ends. After a merge-first merge agents wait
   for the staging verdict in such a shell (tasks #608 and #613 of this project polled
   `yulius tool cd_status` in a background loop), because `yulius wait` follows only the checks
   at the head, the delivery and the task's own CD runs. The staging deploy's CD run was recorded
   as the person's, not the task's (finding 2 below), so even `yulius wait` could not see it end.

The shell waited for news that Yulius had decided never to type, and the task could not read
Done while the shell ran. Nothing ended the stand-off but a person.

The transcript of quartio-v2 #377 is not readable from this project, so the shell's exact command
is inferred from the incident's facts (one background shell, no turn after 11:12:36) and from the
same pattern in this project's tasks #608 and #613.

### Paths checked

- **Held message (10:56).** The hold "someone typed in the terminal and sent no prompt since" was
  resolved at 10:56:09 and the task worked and submitted turns until 11:12:36, so no draft held
  the box later. The outcome was never offered to the session at all (rule 1), so no hold was
  involved. A held message would still defer the new delivery to a later tick (`still_due`).
- **Background shell.** The blocker of Done (rule 2).
- **Idle task.** Only an idle run's background work is probed and counted. A finished task mid-turn
  or flagged needs-attention is decided once it is idle again (review round 1: deciding it
  silently at such a tick lost the outcome a hold had deferred).
- **Claude Code versus Codex.** `machine_notice_tasks`, the background probe (Claude Bash
  wrappers and Codex PTY commands alike, `yulius_ptyhost::shells`) and `type_into_session_if`
  (Codex observed-attempt paste) are harness-agnostic, so the stand-off and the fix apply to both.

### Smaller findings

1. #377's delivery lists review round 2 (yulius-reviewer, Fable 5.2, head 9077a21e, started
   10:16:39) with no `ended_at`, `reported: false` and no verdict, while its notes say round 2
   approved. A round ends only on the reviewer's `SubagentStop` (`review_subagent_stopped`);
   finding why it never matched needs that session's hook and transcript records, which this
   project cannot read. Filed as #1726.
2. A staging request's deploy runs `cd_start` with `CdActor::Person` (the owner's write access),
   so the CD run records `requested_by` as the person's login and no `task_id`, although its note
   says "a task's delivery". Before merge-first a task's deploy read "task #N". Fixed here.

## Requirements

- SWK-001 When a task's staging request ends (verified, included, gate_failed, deploy_failed or
  superseded) and the task is running, idle, accepts no machine notice (finished), and its agent
  still runs background work (a background shell, agent or Monitor, or a `yulius wait`), Yulius
  types the outcome into its session once, saying the task's work is finished and that it stops
  a background command of its that waits for this outcome (and leaves anything else running)
  and answers in one line. The usual holds apply (a person's possible draft, a needs-you request,
  a pause, an update), and a held outcome is retried on a later tick. A finished task that is
  not idle (mid-turn, or flagged by such a hold) is decided at a later tick once it is idle,
  since that turn or hold can still end in a wait. Right before the paste Yulius checks again
  that the task is still finished and its background work still runs.
- SWK-002 A finished idle task with no background work, a parked or ended task keeps receiving
  its outcome silently on its delivery (SPEC-612 GF-006).
- SWK-003 Once that agent's turn ends with no background work left, the merged task with no
  declared work left shows Done (existing rule, TSK-014, SPEC-491 DON-004).
- SWK-004 A CD run started for a task's staging request records that task (`task_id`) and is
  requested by `task:<number>` (shown "task #N"), while the deploy still acts with the task
  owner's access as before. A restore run after a failed deploy stays the owner's.

## Plan

- [x] Root cause of #377's missing Done recorded in the spec
- [x] Regression test replays #377: finished task, waiting shell, staging outcome
- [x] A finished task whose background work waits hears its staging outcome
- [x] Staging deploys are recorded as the requesting task's CD runs
- [x] Decision, docs and changes entry; issue for the unended review round
- [x] Independent review approved

## Acceptance criteria

- A merged task with a complete plan, idle with a background shell, gets each of a gate_failed,
  a deploy_failed and a verified outcome typed into its session once; once the shell ends and the
  turn is over, the task is in the Done group with no person acting.
- An outcome that ends while the finished task is mid-turn or flagged needs-attention is not
  recorded silently: it is typed at the first tick the task is idle with its wait running.
- An outcome whose wait ended between the decision and the paste is not typed and is recorded
  silently at the next tick; the task is Done.
- A staging request's CD run reads `requested_by = task:<n>` and carries the task id; `yulius
  wait` of that task sees the run.

## Test plan

- `task_done::a_finished_tasks_wait_hears_its_staging_outcome_and_the_task_ends_done` (new, real
  `yulius ptyhost` and fake Claude Code agent): replays #377 with a `/bg` shell standing in for
  the agent's staging wait, the three final states; an outcome held by a needs-attention flag
  and by a turn ("sr-held"); the wait ending between the decision and the admission at race
  point `AwaitedOutcomeDecided` ("sr-race"), a silent record and Done. Fails before the fix: the
  outcome is never typed.
- `staging::a_staging_deploy_is_the_requesting_tasks_cd_run` (new): the deploy's CD run record.
- Rerun the touched test binaries' files in full: `task_done`, `staging`, `parking` (staging
  outcome tests), the `yulius-app` unit tests; `scripts/dev quick`.

## Decisions

- Type the outcome only while the finished task's own background work runs, instead of always
  waking finished tasks (ADR-0450 keeps them quiet) or showing Done over a running shell (the
  shell would wake the agent later anyway, contradicting Done). The agent's background work is
  what keeps it from Done; the outcome is the news that work waits for. ADR-0466.
- A fresh look at the agent's processes decides (`background_or_wait_now`), falling back to the
  last probe when it gives no answer, so a stale count neither wakes a finished task nor
  leaves the wait running.
- `yulius wait` following a staging request is left out of this change (#1727): with the
  outcome typed, any wait ends. The CD-run attribution makes its existing CD fields see the
  staging deploy.

## Evidence

- Before the fix, the new `task_done` regression test failed at `the finished task's waiting
  agent hears sr-gate` (left 0, right 1): the outcome was recorded without being typed. After
  it, the test passes (about 2 s; the reviewer ran it with `--stress-count 20`, 20 of 20).
- `staging::a_staging_deploy_is_the_requesting_tasks_cd_run` failed before its fix
  (`requested_by` left "alice", right "task:1") and passes after.
- Hand mutations, each restored: making every finished task "awaited" failed the test at
  `recorded`; removing the not-idle deferral failed it at `a NeedsAttention task's outcome
  waits`; removing the admission recheck failed it at `its work ended before the admission`.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^agent_hooks::/) |
  test(/^disk::/) | test(/^delivery::/) | test(/staging/) | test(/^task_done::/) |
  test(/^parking::/))'`: 661 passed. With `cd::` and `task_message::`: 434 passed (the one
  failure in between was the quiet-receipt test's finished tasks seeded mid-turn; they are now
  idle, 62 of 62 staging tests pass). `scripts/cargo nextest run -p yulius-app`: 723 passed.
  `scripts/cargo clippy -p yulius-app -p yulius --all-targets -- -D warnings`: clean.
  `scripts/dev quick`: passed.
- Yulius CI run 28dabb5e at 7509bb86f passed (quality, lint, macos, docs, deps,
  file-contracts); the test shards and e2e run on the merged revision (merge-first).
- Independent review (yulius-reviewer, Fable 5.2): round 1 requested changes (a hold the
  outcome raised made the next tick drop it; the mid-turn case; the prompt's wording; the
  untested admission recheck; nits), all answered; round 2 approved 314816387.
- Security scan: Vidoc is switched off for this project, so `yulius security-scan` scans
  nothing (disabled, not passed).
- Not tested: a Codex session (the decision, the probe and the paste are shared code; only the
  fake Claude Code agent ran); a real person's draft raising the hold (the test writes the
  flagged activity, the flag-raising code is unchanged); the incident's own transcript, which
  this project cannot read, so the shell's command is inferred.
