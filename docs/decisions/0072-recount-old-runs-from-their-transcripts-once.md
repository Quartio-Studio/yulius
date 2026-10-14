# 0072. Runs recorded before SPEC-087 are recounted from their transcripts once, after the upgrade

## Status

Accepted. Specified in [SPEC-091](../../.ai/specs/SPEC-091-recompute-the-metrics-of-runs-recorded-b.md).
Follows [0070](0070-run-metrics-come-from-the-transcript.md).

## Context

ADR-0070 made a run's tokens, turns and active time a function of its transcript lines, and left
the rows saved before it as they were. Those rows are wrong (production, 2026-10-14: 41 of 109
runs with 0 active time, 17 of 104 without tokens, no sub-agent tokens, 1,991 turns for 882), and
the dashboard's history shows them. Their transcripts mostly still exist in the sandboxes' homes.
The deploy time of SPEC-087 is different on every instance and recorded nowhere.

## Decision

1. **Migration 23 queues every run recorded so far** in `metrics_recounts`, in the same
   transaction that raises the schema version: exactly the runs before the upgrade, once.
2. **After start, a background job recounts each queued run** once reconciliation has ended the
   runs that did not survive the restart. A run's lines are its window of the session transcript:
   from its start (the first line of a new session; the first line at or after the start of a
   resumed run, as ADR-0070) to the first line at or after the earlier of its end and the next
   start of a run of its task on the same session; its sub-agent transcripts over the same window.
   The window is read with the live watch's accumulator, so the figures are those a run gets
   today.
3. **Each run's outcome is recorded with the figures it replaced and the ones stored**
   (`recomputed`, `no_transcript`, `no_lines`, `live`, `gone`), and one audit entry sums the round.
   A recorded run is never recounted; nothing replaces figures with nothing.

## Alternatives

- **A cut-off at SPEC-087's production deploy (2026-10-14 00:18 UTC).** Right on one instance,
  wrong on every other; recounting a run SPEC-087 already counted gives its figures back anyway.
- **Recount in the migration.** The store would read transcripts and parse Claude Code's format:
  the store must not do filesystem or harness work, and start would wait on every file.
- **A one-shot CLI command a person runs.** The request asks Yulius to do it by itself; a command
  is forgotten on some instances and run twice on others.
- **An instance flag instead of a per-run queue.** A crash midway would recount everything, and
  nothing would say what changed per run.
- **Read each run's lines to the end of the file.** A later run of the same session, or a person's
  terminal on it, writes to the same file after the run ended: its lines would count twice.

## Consequences

- The dashboard's history becomes true for every run whose transcript still exists, on the first
  start after the upgrade, without anyone acting.
- The first start reads every recorded run's transcript once, in the background (sequentially, off
  the async workers).
- Runs whose transcripts are gone keep their old figures, and `metrics_recounts` says which.
