# 0117. A run that ends while Yulius is down is counted from its transcript

## Status

Accepted. Specified in [SPEC-143](../../.ai/specs/SPEC-143-fix-385-389-a-run-that-ends-while-yulius.md);
task #124, fixing #385 and #389. Follows [0070](0070-run-metrics-come-from-the-transcript.md) and
[0072](0072-recount-old-runs-from-their-transcripts-once.md).

## Context

A run's figures are saved from the usage snapshot its transcript watcher builds in the running
process (ADR-0070). When a run's agent exits while Yulius is down, the next start's reconciliation
finds its exit record and ends it. No watcher of that process ever read the run, so its snapshot is
empty, and saving it replaced the run's stored tokens, turns and active time with NULLs (#385).
The SPEC-091 recount covered the upgrade that introduced it, not later restarts.

## Decision

- **A run its watcher has not read to the end is counted, not saved from the snapshot.** The
  runtime names the run whose transcript its watcher read to its end at least once (`usage_read`).
  When any other run ends — reconciliation finds it exited, or a reattach gets its exit before the
  watcher's first poll (its host lingers 30 s to hand the exit over) — its window of its session
  transcript is counted with SPEC-091's recount (the live watch's accumulator, sub-agents
  included) once its end is recorded, and the stored figures are replaced only by figures it
  found. No transcript, no line of the run, or a failed count leaves the row as it was. A run its
  watcher has read is saved from the snapshot, as before.
- **Recovery is one step that reports the recount.** `App::recover` reconciles and starts the
  metrics recount only when reconciliation succeeded, returning its handle; `start_background`
  runs it, so a test can prove the order (#389).

## Alternatives

- **Skip the save for a run nobody watched.** Simplest, and never worse than before, but the
  stored figures are those of the last live save, which miss what the run did after it: a turn or
  a goal continuation while Yulius was down, a turn the 30 s live throttle had not saved. Kept as
  the fallback when the count finds nothing.
- **The caller says whether a watcher read the run** (`Watched`/`Unwatched`, the first round of
  this change). Reconciliation knows, but the reader of a reattached run does not know whether its
  watcher has polled yet, so it saved an empty snapshot when the exit came first (review round 1).
- **Count whenever the snapshot is empty.** Misses a partial snapshot (a long transcript replayed
  over several polls) and cannot tell whose snapshot it is.
- **Queue such runs in `metrics_recounts`.** Reuses the queue, but delays the figures past start
  and mixes a per-restart event into a once-per-upgrade record.

## Consequences

- A restart never erases a run's usage; a run that worked while Yulius was down gets those lines.
- Each run that ends before its watcher read its transcript costs one read of its transcript
  window when it is ended, off the async workers (`spawn_blocking`) but on the path that ends it:
  at start, reconciliation counts such runs one after another before the queue drains and armed
  merges are restored, so a long outage with many large transcripts delays them by those reads
  (review round 2, accepted: a read of a transcript is short next to a restart).
- A counted model replaces the stored one, as a live save does; the stored model comes from the
  same window's assistant lines, so the count finds it whenever the row has one.
- A run that exits before its watcher's first poll (a spawn failure, an exit within a second) is
  counted too: no metrics row when its transcript has nothing, rather than a row of NULLs.
