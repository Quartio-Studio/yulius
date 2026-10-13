# SPEC-087: every time and figure says what it measures, and the figures are the agent's real work

Status: implemented
Date: 2026-10-13
Owner: engineer E of the 2026-10-13 round
Scope authorization: the person's request: "time shown next to the tasks + data in dashboards
doesn't seem real often. Verify it for ambiguity." Fix wrong numbers at their root, in recording
or aggregation, and make every shown time unambiguous.

## Goal

A person reading a time or a number in Yulius knows what it measures, and the number is true:
- next to a task, how long it has been in the state they see ("working 1h 20m", "waiting 12m",
  "done 3h ago"), with the exact times on hover;
- on the dashboard and a task's overview, the agent's working time, tokens and turns as its
  Claude Code session recorded them, through restarts, resumes and sub-agents.

## Context

Measured on the instance on 2026-10-13 (database, the sandbox's transcripts, the journal):

- **The time next to a task was the last change of anything.** The sidebar showed
  `relShort(updated_at)` with no word. `updated_at` moves on every status or activity write,
  also when the value does not change (every prompt sets Working), and on title, owner, archive and
  other edits. At 13:30 UTC the eight Working tasks showed "1m, 2m, 3m, 3m, 6m, 20m, 26m, 29m": the
  time since their last wake-up by their own background work. Their transcripts show each had
  worked without a person since its start, 95 to 406 minutes earlier. Task #68 showed "1m", having
  worked since 11:32 (1 h 58 min). The chained tasks' "2h, 7h" was the time since they were made.
- **Active agent time was kept in memory and lost.** It was summed from in-memory turn starts and
  ends, reset to zero whenever a run's watcher started, and written over the stored value. A
  restart (22 on 10-12 and 10-13, mostly deploys) zeroed every live run's active time. 41 of 109 runs
  since 10-11 store 0 although their transcripts show work: the dashboard's "Active agent time" for
  them was 27.6 h; their transcripts show 95.8 h from prompts to answers, 110.8 h of Claude Code's
  turns. The in-memory measure also ended a turn at its first answer, so time a turn spent waiting
  for its own background agents never counted.
- **A resumed run started from the end of its transcript.** A resumed session writes to the same
  file as the run before it. The watcher opened it at its end when it first polled, about a second
  after the start, missing the lines before. After a restart it opened at the end again, then
  saved its empty totals over the run's. 17 of 104 runs store no tokens although their transcripts
  show work, for example task #70's second run: 86,508 output tokens and 115 minutes of turns.
- **Sub-agents' tokens were not counted.** Claude Code 2.1.292 writes each sub-agent's
  conversation to `<session>/subagents/agent-<id>.jsonl`; none of its messages is in the session's
  file. They hold 3.49 M of 10.2 M output tokens (34 %) of these runs.
- **Turns were every answer.** Each `end_turn` message counted, sub-agents' and goal continuations'
  included: 1,991 turns stored where Claude Code recorded 882 (task #8: 537 for 39).
- **The dashboard's durations said less than they meant.** "Median duration" was the median time a
  task's sessions stayed open, idle waiting for a person included (task #30: 13 h open, 7 h of
  turns). A running task whose goal is achieved counted as "Running" in the outcomes. Days are UTC
  dates, which the page did not say.
- Already fixed before this change: a switched run's old watcher kept saving the next run's figures
  under its own run (#335, `watcher_generation`); rows saved before it stay as they are.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MET-001 | A run's active agent time comes from its transcript alone: the union of its turns' spans, in the transcript's times. A turn runs from its prompt (or the agent's first work after a turn's end, a Stop hook continuing it) to its end: the session's `end_turn` answer, an interrupt, a usage limit or Claude Code's turn record; a turn record also covers its `durationMs` span, which includes the turn's wait for its own background agents. A turn in progress counts up to the newest line read. Idle time waiting for a person never counts. The spans kept are bounded (1,024; older ones settle into the total). | `observe::tests::active_time_is_the_union_of_the_turns_spans`, `idle_time_between_turns_is_not_active`, `an_open_turn_counts_to_the_newest_line_and_an_interrupt_or_limit_ends_it`, `a_continued_turn_stays_active_and_counts_once`, `active_span_memory_is_bounded`; `run_metrics::a_restart_keeps_a_resumed_run_s_metrics` |
| MET-002 | A run's figures come only from its own lines, and the same ones whenever they are read. A new session's run reads its transcript from the start; a resumed run reads its session's transcript from the first line recorded at or after its start, both when watched from its launch and when reattached after a restart. A restart therefore neither resets nor overwrites the tokens, turns or active time a run had. | `subagents::tests::lines_since_keep_the_run_s_own_lines`; `run_metrics::a_restart_keeps_a_resumed_run_s_metrics` (fails with the old open-at-end watch) |
| MET-003 | A run's turns are Claude Code's turn records (`turn_duration`); from a CLI that writes none, its session's `end_turn` answers. A sub-agent's answers never count. | `observe::tests::a_continued_turn_stays_active_and_counts_once`, `counts_malformed_lines_turns_and_keeps_the_latest_limit` |
| MET-004 | A run's token usage includes its session's sub-agent transcripts (`<session>/subagents/*.jsonl`, lines from the run's start for a resumed session): their usage only, never their model, limits or turns. At most 256 files, regular files only (no symbolic links), lines of at most 1 MiB; an unreadable one leaves the session's own observations alone. | `observe::tests::a_sub_agents_lines_add_only_their_tokens`; `subagents::tests::sub_agent_files_are_found_and_read_from_the_run_s_start`, `symbolic_links_are_not_followed`; `run_metrics::a_restart_keeps_a_resumed_run_s_metrics` |
| MET-005 | The analytics summary adds `duration_ms.median_active` and `p90_active`, over the tasks whose runs reported active time (their runs' sum; unknown is not zero). `tasks.by_status` counts a running task whose goal is achieved under `done`. The dashboard shows "Agent work per task" (that median), "Agent working time" with "sessions open …, idle included", tokens "sub-agents included", outcomes "done or closed", and says its days are UTC. | `analytics::tests::work_per_task_is_the_median_of_the_tasks_active_time`; `analytics::summary_sums_known_usage_counts_unknown_runs_and_hides_invisible_sandboxes`; e2e `times.e2e.ts` "a task overview and the dashboard name what each figure measures" |
| TIM-001 | A task has a state time (`state_at`, `TaskSummary.state_since`): the last change of its status or activity. Setting the same value again, or changing anything else, leaves it; a new task's is its creation. Existing tasks take their last change. | `store::the_state_time_moves_only_when_the_state_changes`; `tests::version_nineteen_tasks_get_their_state_times` |
| TIM-002 | A task has a working stretch (`working_since`): it starts when the task starts working or is on its way (starting, continuing on another subscription, paused for capacity, running and working); it goes on through idle turns while background work wakes the task, and through subscription switches; it ends when the task needs a person, is queued or chained, or stops, closes or fails, and when an idle task has no background work left. While an idle task's background work is not known yet (its turn just ended, or Yulius restarted), nothing changes. A live task that does not wait for a person migrates with its first run's start. | `store::the_state_time_moves_only_when_the_state_changes`; `tasks::tests::background_work_decides_an_idle_task_s_working_stretch`; `tests::version_nineteen_tasks_get_their_state_times` |
| TIM-003 | Every time next to a task says in words what it measures, by the group the person sees, with the exact times and the creation time on hover: Working "working 1h 20m" (its stretch), Needs you "waiting 12m", queued or chained "queued 2h", paused "paused 5m", in review "in review 3m" (since its turn ended), Done "done 3h ago", Stopped "stopped / closed / failed 2d ago". Lengths are rounded down. The sidebar, the Tasks page (columns "Created" and "For") and a task's overview ("Created", "Now", "Agent work … sessions open") use one rule. Session lists say which time they show ("ended", "imported", "started"); a CI run says "Started" or "Queued" with both times on hover. | e2e `times.e2e.ts` "each group says what its time measures, in words (unit, in the page)", "the sidebar and the Tasks page say how long a task has worked, not when it last changed", "a task overview and the dashboard name what each figure measures" |

## Plan

- [x] Spec written and validated
- [x] Active time, turns and sub-agent tokens from the transcript (harness)
- [x] A resumed run reads its own lines, at launch and after a restart (supervisor, wiring)
- [x] State time and working stretch stored and published (store, app, contracts)
- [x] Analytics: per-task active time, `done` outcome
- [x] UI: one rule for task times; sidebar, Tasks page, overview, dashboard, sessions, CI
- [x] Docs (API, changes, ADR), self-review, gates

## Acceptance criteria

- The integration test restarts Yulius under a resumed run with a sub-agent: its tokens, turns and
  active time are the same after the restart; with the old open-at-end watch it fails.
- A task that works for hours, woken by its own background work, reads "working 2h 14m" in the
  sidebar and on the Tasks page, not the time since its last wake-up.
- `cargo fmt`, `clippy -D warnings`, `scripts/check_repository.py`, the touched and neighbouring
  test suites and the e2e suite pass.

## Test plan

- Unit: `yulius-harness` observe (active spans, turns, sub-agent usage), subagents (lines since a
  time, discovery bounds); `yulius-app` tasks (stretch rule), analytics (per-task active, outcome
  key); `yulius-store` (state time and stretch in updates; migration 20).
- Integration (fake agent, real ptyhost): `run_metrics` (switch to a resumed run, a sub-agent file,
  restart); the existing `pool_supervision`, `task_done`, `context_full`, `supervision`,
  `recovery`, `shells`, `analytics`, `sessions` suites unchanged and green.
- e2e (mock): `times.e2e.ts`.

## Decisions

- **The transcript is the record of a run's work.** Active time, turns and tokens are computed
  from the transcript lines the run wrote, never from Yulius's in-memory view of turns, so reading
  them again (a restart) gives the same figures. ADR
  [0070](../../docs/decisions/0070-run-metrics-come-from-the-transcript.md).
- **Active time is the union of turn spans.** Claude Code's `durationMs` of a turn that waited for
  its background agents reaches back to the prompt that started them, so summing it counts the
  same time several times; the union counts it once and includes the wait, which is work the
  person sees as Working.
- **A resumed run's lines are found by time.** Its first line is the first one recorded at or after
  the run's start (same host clock); no offset is stored. A line the previous process wrote in the
  second after the new run started counts for the new run.
- **The working stretch is kept by the server.** The client cannot know what happened before it
  loaded. Status and activity changes keep it in the store; the end of an idle task's background
  work is decided where Yulius probes it and publishes the task, and an unknown count changes
  nothing, so the brief moment between a turn's end and its background count does not end it.
- **Historic rows stay.** Runs saved before this change keep their figures; recomputing them from
  transcripts would rewrite the production database.

## Evidence

- Instance analysis (read-only, 2026-10-13): the numbers in Context, from `metrics`, `attempts` and
  the transcripts of the 72 tasks since 2026-10-11, and the 13:30 UTC reconstruction of the Working
  group from the transcripts' prompts and turn records.
- `run_metrics::a_restart_keeps_a_resumed_run_s_metrics` passes; with the watcher opening a
  resumed run's transcript at its end (the old behaviour) it times out waiting for the run's active
  time, before the restart even happens.
- Unit tests above; `cargo nextest` of `yulius-harness`, `yulius-store`, `yulius-app` and the
  `yulius` suites named in the test plan pass; e2e `times.e2e.ts` and the whole e2e suite pass.
