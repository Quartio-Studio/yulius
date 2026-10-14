# 0070. A run's metrics come from its transcript; a task's times from its state changes

## Status

Accepted. Specified in [SPEC-087](../../.ai/specs/SPEC-087-times-and-metrics.md).

## Context

The dashboard's and a task's figures, and the time next to each task, did not match what the
agents did (measured on the instance, 2026-10-13):

- Active agent time was summed in memory from Yulius's view of turns. Each watcher start reset it
  and the next save wrote it over the stored value, so every restart zeroed the live runs' active
  time: 41 of 109 runs stored 0, and the dashboard showed 27.6 h where the transcripts held 110.8 h
  of turns.
- A resumed run's watcher opened its session's transcript at the end, a second after the start
  and again after a restart, then saved its empty totals over the run's (17 runs lost their
  tokens). Sub-agents' transcripts (34 % of output tokens) were never read, and every `end_turn`
  answer counted as a turn (2.3 times too many).
- The time next to a task was the time since its last change of anything. A task woken by its own
  background work a minute ago read "1m" after hours of work.

## Decision

1. **A run's tokens, turns and active time are computed from its transcript lines only.** The
   accumulator that sums tokens also keeps the turns (Claude Code's turn records) and the active
   time: the union of the turns' spans in the transcript's own times, a turn in progress up to the
   newest line. Yulius's in-memory turn state no longer feeds any figure.
2. **A run always reads the same lines: its own.** A new session's run reads its transcript from
   the start; a resumed run from the first line recorded at or after its start, whether watched
   from its launch or reattached after a restart. The run's sub-agent transcripts count from the
   same time, their usage only. A save after a restart therefore repeats the run's figures.
3. **A task's times are kept by the server, by meaning.** `state_at` moves only when the status or
   activity changes value. `working_since` is the start of the stretch in which the task has not
   waited for a person: status and activity changes start and end it in the store, and an idle
   task's end of background work ends it where Yulius probes that work and publishes the task. The
   UI shows, per group, the one measure that answers the person's question, in words.

## Alternatives

- **Persist the in-memory active time and add to it after a restart.** Lines written while Yulius
  is down, and since the last save, would be lost, and a resumed run's own earlier lines would
  stay unread.
- **Store the byte offset where a resumed run's lines begin.** Exact too, but needs a schema change
  and its update at every launch and session change; the lines' own times give the same boundary
  on one host.
- **Sum Claude Code's `durationMs`.** A turn that waited for background agents reports the time
  since the prompt that started them, so consecutive turns overlap and the sum counted 176 h for
  111 h of turns.
- **Compute the time in the browser from the last activity change.** A client cannot know what
  happened before it loaded, and every viewer would disagree after a reload.

## Consequences

- Restarts and resumes no longer change a run's figures; live figures can lag by one line while a
  long tool call runs, and catch up with the next line.
- Reading a resumed session's transcript up to the run's start costs a scan of the file once per
  watch (bounded per poll as before).
- Rows saved before this change keep their figures; the dashboard is right for runs from now on.
  [ADR-0072](0072-recount-old-runs-from-their-transcripts-once.md) recounts them once from their
  transcripts.
