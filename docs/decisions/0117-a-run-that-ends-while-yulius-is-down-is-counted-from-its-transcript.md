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

- **Reconciliation counts the run instead of saving the snapshot.** Once the end is recorded, the
  run's window of its session transcript is counted with SPEC-091's recount (the live watch's
  accumulator, sub-agents included), and the stored figures are replaced only by figures it found.
  No transcript, no line of the run, or a failed count leaves the row as it was.
- **The caller says how the figures are known.** `finalize_attempt` takes `Watched` (the run's
  watcher in this process holds them: unchanged live path) or `Unwatched` (reconciliation).
- **Recovery is one step that reports the recount.** `App::recover` reconciles and starts the
  metrics recount only when reconciliation succeeded, returning its handle; `start_background`
  runs it, so a test can prove the order (#389).

## Alternatives

- **Skip the save for a run nobody watched.** Simplest, and never worse than before, but the
  stored figures are those of the last live save, which miss what the run did after it: a turn or
  a goal continuation while Yulius was down, a turn the 30 s live throttle had not saved. Kept as
  the fallback when the count finds nothing.
- **Infer "unwatched" from a runtime flag.** The runtime is per task and outlives its runs, so a
  flag would need resetting at every launch and reattach; the one caller that knows says it.
- **Queue such runs in `metrics_recounts`.** Reuses the queue, but delays the figures past start
  and mixes a per-restart event into a once-per-upgrade record.

## Consequences

- A restart never erases a run's usage; a run that worked while Yulius was down gets those lines.
- Each run that ended while Yulius was down costs one read of its transcript window at start, off
  the async workers.
- A counted model replaces the stored one, as a live save does.
