# SPEC-085: a task's status tells the truth: Working is work, Needs you is a person's move

Status: implemented
Date: 2026-10-13
Owner: engineer C of the 2026-10-13 round (branch `fix/status-truth`)
Scope authorization: the person's report of 2026-10-13: "status message broke. It was marked as
running even if was done" and "needs you was too often", with the screenshots of task #60
(20:01 UTC) and task #63 (19:56 UTC).

## Problem and evidence

"Needs you" must mean that a person has to act: a question or permission prompt, an agent that
stopped and cannot go on by itself, gate exhaustion. It must never mean work the agent does or
will do by itself. "Working" must mean the agent works or will be woken by its own work.

Every task transcript of the sandbox from 2026-10-11 22:00 to 2026-10-13 20:30 UTC (69 tasks, 975
idle periods between a turn's end and the agent's next work) was replayed against the dev rules
(SPEC-079 included), with the pull requests' CI runs, merges and plan steps from the database.
Each idle period was sampled every 15 s for what the sidebar showed, and compared with how it
really ended: the agent woken by its own work (a `<task-notification>`, Claude Code's goal
check-in) or by a person's prompt.

| Cause | Shown | Periods | Task time | Tasks |
| --- | --- | --- | --- | --- |
| The agent waits in `yulius wait` for its checks or armed merge; the probe leaves such a shell out of the background count (SPEC-050 DSK-102) and the plan's merge step is still open, so the pull request's "In review" does not apply (#139) | Your turn (Needs you) | 173 | 43.1 h | 37 |
| Failing checks while a background agent or shell runs or the agent waits in `yulius wait`, and it fixes them itself (task #63's screenshot: round-3 review agent pending, flake fixed, head pushed) | PR checks failing (Needs you) | 20 | 20.5 min | 5 |
| A record Claude Code writes for itself read as a prompt: the summary of an idle compaction (task #60's screenshot) | Working | 1 | 19 min, until the person stopped it | 1 |
| The same, `/compact` typed into a full session (task #8) | Working | 1 | 2.4 min | 1 |

Already fixed on dev, not counted above: before SPEC-079 (deployed 2026-10-13 20:21 UTC) each of
the 853 "not met" goal verdicts left the task idle (Your turn) for the evaluator's 4.9 s (median;
p90 6.5 s) before it read Working again. No CI run errored on these days, so SPEC-050's "CI could
not run" rule (#352) changed nothing here.

Task #60, from its transcript: the goal was achieved at 18:48:21 and the turn closed. At
19:42:51 Claude Code compacted the idle session: a `compact_boundary` record, the user record
with the summary (`isCompactSummary`, `isVisibleInTranscriptOnly`, string content "This session
is being continued…"), four `file` attachments ("Read …/tasks/<id>.output"), a
`compact_file_reference` (finish.rs), the `SessionStart:compact` hook and the notice "Compacted
while idle, before the prompt cache expired". The harness read the summary as a typed prompt:
the task became Working, its achieved goal was dropped, and no Stop hook or turn record ever
ended that "turn". The sidebar's "19m" at 20:01 is the time since 19:42. The text typed into the
prompt box was never submitted (no record).

Task #63, from its transcript: at 19:56:38 its turn closed with `pendingBackgroundAgentCount: 1`
(the round-3 review agent) and no goal verdict (Claude Code defers it while background work runs,
and checked in at 19:57:38). The pull request's head e793eab had failed at 19:34:51 (a test
flake the agent had fixed). The task read Waiting with one background agent counted, and the
sidebar's rule put failing checks before background work: "Needs you".

Measured but left as they are:
- In 57 idle periods a background shell ended right before Claude Code's notification woke the
  agent. The replay's 15 s samples show them as Your turn for 15 to 30 s; in Yulius the wake-up's
  `UserPromptSubmit` hook makes the task Working within a fraction of a second, and the 5 s shell
  probe rarely falls into that gap.
- Leftover shells under achieved goals (`sleep 600`, a preview server) keep a task Working, as
  TSK-014 says.

## Scope

In scope: the transcript reading of user records, the process probe's `yulius wait` shells, the
idle task's view, the sidebar rule (`status.js` `taskInfo`) and its server twin
(`AttentionFacts::group`, SPEC-076). Not in scope: notices that ask a person (STA-002), gate
exhaustion, held prompts and browser requests, which stay "Needs you".

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| STT-001 | A user record Claude Code writes for itself starts no turn: the summary a compaction continues from (`isCompactSummary` or `isVisibleInTranscriptOnly`), a slash command's echo (`<command-name>`, `<command-message>`), a local command's caveat and output (`<local-command-caveat>`, `<local-command-stdout>`, `<local-command-stderr>`) and the shell mode's records (`<bash-input>`, `<bash-stdout>`, `<bash-stderr>`). It still names the session; a failed compaction's output stays a failed compaction. An idle compaction leaves a Done or waiting task as it was. The model's own work after a command (`/goal`) shows its turn. | `claude::tests::records_claude_code_writes_for_itself_start_no_turn`, `observe::claude_goal_mode_records`, `task_done::an_idle_compaction_leaves_a_done_task_done` |
| STT-002 | The process probe counts background shells whose only process is `yulius wait` apart from the others, which DSK-102 keeps out of the background count (disk upkeep, DLV-203, Done and the session limit are unchanged). The view of an idle task says so (`yulius_wait`). An idle agent waiting there goes on by itself: its waiting task reads "Waiting for checks or merge" in the Working group, with the tone of Working (no notification), whatever its plan or pull request say. An achieved goal stays Done. | `shells::tests::a_shell_that_only_runs_yulius_wait_does_not_count`, `task::tests::an_agent_that_goes_on_by_itself_needs_nobody`, e2e `task-done.e2e.ts` "an agent waiting for Yulius is Working, not your turn" |
| STT-003 | Failing checks need a person only once the agent stopped: while the idle agent's background work runs (shells, pending agents, live monitors) or it waits in `yulius wait`, the task reads Working with its pull request ("Working · PR #N checks failing"). Once nothing runs, failing checks need you as before. A permission prompt needs you whatever runs. | `task::tests::an_agent_that_goes_on_by_itself_needs_nobody`, e2e `task-done.e2e.ts` "failing checks wait for the agent's own work before they need you" |
| STT-005 | `/compact` typed into the session (by a person, or by Yulius for a full context, CTX-003) starts no work: Claude Code 2.1.292 writes the command as typed (a user line `/compact [what to keep]` with no `origin`), compacts, then writes the boundary, the summary, the command's caveat, echo and output, and runs no turn and no Stop hook. The transcript reads no turn from that line, and a `UserPromptSubmit` of it, should Claude Code run one, neither sets Working nor takes back an achieved goal. The task stays as it was during and after the compaction; a prompt typed after it starts work as always. | `harness::tests::the_compaction_command_is_known_as_typed`, `claude::tests::records_claude_code_writes_for_itself_start_no_turn`, `finish::tests::the_compaction_command_starts_no_work`, `task_done::a_typed_compaction_leaves_a_done_task_done` |
| STT-006 | While Yulius recovers a full session by itself (SPEC-060: it compacts it, or continues the task in a fresh session), the task reads Working, not Your turn: nobody has to act. This lasts from the refusal until Yulius types the prompt that carries on (whose turn is Working as any) or the fresh session starts. When Yulius types nothing (a person typed into the session, the terminal never went quiet, the input failed), the task waits for the person (Your turn), unless a prompt of theirs started a turn. A recovery that fails needs a person as before (CTX-004, CTX-005). CTX-002 says so. | `context_full::a_full_session_reads_working_while_yulius_compacts_it`, `context_full::a_full_session_a_person_types_into_waits_for_them`, the other `context_full` tests |
| STT-004 | The sidebar, the task header, the attention bar, notifications and the Home and tab counts keep one rule: `taskInfo` in `status.js` and `AttentionFacts::group` give the same group for the same facts (ADR-0061). | `task::tests::group_follows_the_sidebar_rule`, `task::tests::an_agent_that_goes_on_by_itself_needs_nobody`, e2e `task-done.e2e.ts` |

## Design and contracts

- **Transcript (yulius-harness `claude.rs`).** `observe_user` checks a record for Claude Code's
  own marks before it makes a `UserTurn`: only `SessionMeta` comes out of such a record. Typed
  prompts (`origin.kind: human`) and Claude Code's wake-ups (`<task-notification>`) are unchanged.
  The replayed final state (`TranscriptFacts`) follows: an idle compaction no longer leaves a
  replayed turn open.
- **Probe (yulius-ptyhost `shells.rs`).** `count_background` returns `ShellCount { shells, waits }`
  from the same walk; `count_shells` and `background_shells` keep their meaning.
  `SessionHost::background_probe` (yulius-app port) returns both; its default (a host that cannot
  tell them apart) reports no waits.
- **Runtime (`finish.rs`).** The idle probe stores `background_waits` next to `background_shells`
  (-1 when not known, cleared with a new attempt) and publishes the task when either changes.
  `background_work_now` (disk upkeep, switches) and `task_is_done` still read the shells alone.
- **View.** `TaskSummary.yulius_wait` (contracts) is true while the task is running, idle and the
  last probe found a waiting shell. `AttentionFacts.yulius_wait` carries it to the server rule.
- **Sidebar (`status.js`).** `statusInfo`: waiting with `yulius_wait` reads `Waiting for checks or
  merge` (group `working`, tone `working`, the `branch` icon). `taskInfo`: failing checks with
  background shells or `yulius_wait` read Working with the pull request. `goesOn(task)` names the
  condition. `activityLine` says "Waits for its checks or merge; goes on by itself".
- **Bounds.** `yulius wait` ends within its own limit (its `--minutes`, the CLI's default), so a
  task never reads Working for good on a wait: when it returns, Claude Code wakes the agent; if
  the agent then stops, the task reads Your turn.

## Failure and compatibility

- A probe that gives no answer reports no waits: the task reads as before (Your turn).
- Older clients ignore `yulius_wait` (`serde(default)`); older servers do not send it and the
  sidebar reads as before.
- `/compact` (STT-005), from the only manual compaction in the VM's transcripts (task #8,
  2026-10-12 04:22:29 to 04:23:25, Yulius's own compaction of a full session): the line
  `/compact` with a `promptId` and no `origin` at the start; at the end the `compact_boundary`
  (`trigger: manual`), the summary, the caveat (`isMeta`) and the `<command-name>/compact` echo
  (both dated when it was typed), the output `Compacted (ctrl+o to see full summary)` and the
  `SessionStart:compact` hook; no `turn_duration`, no Stop hook. No `UserPromptSubmit` context
  was recorded for the line, while the prompt before it (70 s earlier, same unread inbox) had
  one, which suggests Claude Code runs no prompt hook for it; STT-005 covers both cases. Before
  STT-005 the line read as a prompt and the task read Working for good.
- Yulius's own recovery of a full session (STT-006) read Working in production only by that
  line, and Your turn in CTX-002 and its test. It now reads Working by decision, from the
  refusal until Yulius types the prompt that carries on, and then for that prompt's turn. A
  recovery Yulius leaves to a person (they typed into the session, the terminal never went
  quiet, the input failed) reads Your turn; one that fails reads Needs you, as before (CTX-004,
  CTX-005).

## Delivery and acceptance

- Unit: the records of task #60's idle compaction and of `/goal` and `/compact`, shapes from the
  VM (`claude.rs`); the probe's waits (`shells.rs`); the server rule (`task.rs`); the compaction
  command as typed (`harness.rs`) and its prompt hook (`finish.rs`).
- Integration: the fake agent compacts its idle session like Claude Code (`--idle-compact-ms`);
  the Done task stays Done and is never published Working. It writes Claude Code's records for a
  typed `/compact` too: the Done task stays Done, and a prompt typed after it is Working
  (`task_done.rs`).
- e2e (mock): a waiting task whose agent waits for Yulius is Working; failing checks with a
  background agent are Working, and need you once it ended (`task-done.e2e.ts`).
- Red first: each fix was removed in turn and its tests failed (Implementation evidence).

## Open decisions

None blocking. ADR [0068](../../docs/decisions/0068-needs-you-means-a-person-has-to-act.md)
records the decision that "Needs you" waits for the agent's own work.

## Implementation evidence

- **Red first.** Each fix was removed in turn and its tests failed:
  - without the transcript fix, `claude::tests::records_claude_code_writes_for_itself_start_no_turn`
    and `observe::claude_goal_mode_records` failed, and
    `task_done::an_idle_compaction_leaves_a_done_task_done` read `Working` instead of `Done`
    (task #60 reproduced);
  - without the rule, `task::tests::an_agent_that_goes_on_by_itself_needs_nobody` failed and
    `task_done::an_agent_waiting_for_yulius_does_not_need_a_person` counted the waiting task as
    needing a person;
  - without the probe's waits, that integration test never saw `yulius_wait`;
  - with the old `status.js`, the two new tests of `task-done.e2e.ts` failed.
- **Gates.** `cargo fmt`, `cargo clippy --workspace --all-targets --all-features -D warnings`,
  `scripts/check_repository.py`; nextest of yulius-app, yulius-harness, yulius-domain,
  yulius-contracts, yulius-ptyhost and fake-agent (634 passed); the integration files
  `task_done`, `context_full`, `session_sync`, `disk`, `agent_hooks`, `projects` and
  `supervision` (94 passed); e2e `task-done.e2e.ts` (12 passed), `consistency`, `home`
  and `archive` (22 passed).
- **STT-005, red first.** Without it, `claude::tests::records_claude_code_writes_for_itself_start_no_turn`
  and `finish::tests::the_compaction_command_starts_no_work` failed, and
  `task_done::a_typed_compaction_leaves_a_done_task_done` read `Working` instead of `Done` after
  the compaction. With it, that test, `an_idle_compaction_leaves_a_done_task_done`,
  `an_achieved_goal_with_no_shell_left_makes_the_task_done` and the `context_full` file pass.
- **STT-006, red first.** With the refused turn settled to Your turn as before,
  `context_full::a_full_session_reads_working_while_yulius_compacts_it` failed (not Working while
  Yulius compacts). Without the hand-over to the person,
  `context_full::a_full_session_a_person_types_into_waits_for_them` timed out waiting for Your
  turn (Working for good). With both, the 8 `context_full` tests pass.
- **Not tested here:** a real Claude Code session against a deployed Yulius; the fake agent's
  compactions are shaped on the records of tasks #60 and #8.

## Changelog

- 2026-10-13: written with the fix.
- 2026-10-14: STT-005, a typed `/compact` starts no work (the gap left open on 2026-10-13);
  STT-006, Yulius's own recovery of a full session reads Working (CTX-002 changed).
