# 0216. CD and environment runs have a disk bound of their own

## Status

Accepted. Specified in [SPEC-300](../../.ai/specs/SPEC-300-cd-and-environment-run-files-count-again.md);
task #286, issue #664. Completes the run bounds of SPEC-134 CDR-004 (time, output) with disk.

## Context

- A CD run (SPEC-134) and a delivery environment run (SPEC-219) write the commit's tree, `HOME`,
  agent states and the log into their own directory (`<data>/runs/{cd,env}-<run>`, or for
  production `<data>/cd/<sandbox>/runs/<name>`), and a CD run writes its persistent cache
  `<data>/cd/<sandbox>/cache/<env>`. These are out of every sandbox directory on purpose (agents
  must not reach them, ADR-0109), so the sandbox's disk quota (SPEC-059) never measures them.
- While a run executed, only its time (the CI job timeout) and its log (256 MiB) were bounded. A
  deploy writing into `HOME` or the cache could fill the data disk until its timeout.
- The sandbox quota is measured over the whole sandbox (worktrees, CI checkouts, Docker storage,
  up to 4 M entries) and cached between readings; it refuses CI jobs at their start (CI-303) and
  exists only with containment on.

## Decision

- Every CD and environment run has one fixed disk bound, an instance setting:
  `--cd-run-disk-gb` (`YULIUS_CD_RUN_DISK_GB`, default 20 GB).
- What counts: the run's files directory, and for a CD run its environment's cache. Both are
  measured every 5 s while the run executes (`ci_watch`), in a blocking task the watch loop only
  polls, with the bounded `dir_size` (allocated blocks, links not followed, at most 4 M entries).
- A run past the bound is killed with its process group and recorded `error`: "the run's files
  used X (run files A, cache B), past the Y disk bound for a run; it was stopped".
- A CD run stopped over the bound has its cache emptied ("; its cache was emptied"), since
  nobody else can, and a cache past the bound would stop every next run of that environment.

## Alternatives

- **Charge the run's files to the sandbox's quota.** A production deploy would then die because
  agents filled their worktrees, the whole sandbox would be walked every few seconds of a run (or
  judged on a stale reading), and an instance without containment has no quota, so its runs
  would stay unbounded.
- **A bound per project in `yulius.toml`.** Agents write that file; the bound protects the
  machine's disk, which is the instance's to decide.
- **A filesystem quota (project quotas, a loop device per run).** A hard limit with no polling,
  but it needs a quota-enabled data filesystem or root for each run, which ordinary installs and
  macOS development hosts do not have.

## Consequences

- No CD or environment run can use more than about the bound plus 5 s of writing on the data
  disk. A deploy that needs more (a large build cache) needs a higher `--cd-run-disk-gb`.
- Measuring costs a walk of the run's files every 5 s; a tree of millions of entries is
  undercounted past 4 M entries, as the sandbox quota is.
- Files a production run's containers wrote under mapped ids count only as far as the server can
  read them.
