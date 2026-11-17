# 0360. Dev's gate judges every change, and a bad yulius.toml never turns CI off

## Status

Accepted. Specified in [SPEC-504](../../.ai/specs/SPEC-504-ci-leaves-nothing-behind-and-a-bad-yuliu.md)
(task #486), CFG-001 to CFG-006. Supersedes SPEC-050 CI-110 and the own-jobs rule of SPEC-163
PCI-002 and SPEC-483 D4 (a change that changed its jobs was tested with its own), and SPEC-483
D4's "nothing is tested while the integration branch's configuration is broken".

## Context

Two production incidents in autoresearch, 16 and 17 Nov 2026:

- PR #4 merged a `yulius.toml` with both `[commands] gate` and `[[ci.jobs]]`. Yulius CI reads the
  gate from `dev` and refuses that pair. From then on it ran nothing for any pull request,
  including PR #5, which fixed it. PR #4's own run had passed: its own file could not be read, so
  it silently ran `dev`'s previous jobs.
- PR #42 merged a valid multi-line TOML array (`paths = [` with one entry per line). Yulius's own
  TOML-subset reader refused it ("yulius.toml line 117: unsupported value"). CI then said "No CI
  is configured" for every pull request and `delivery_merge` refused. With a GitHub ruleset that
  requires `yulius/ci`, the one-line fix (PR #50) could merge only through a person's bypass.

The rules allowed both incidents:

- A pull request that changed its jobs was tested with its own (CI-110), so it could weaken the
  gate that judged it.
- A file that could not be read fell back to `dev`'s jobs without a word.
- `dev`'s broken file turned CI off for every pull request.
- The reader understood only a subset of TOML.

## Decision

- **A real TOML parser** (CFG-005). `yulius.toml` is read with the `toml` crate's spanned
  document (TOML 1.0, `parse` feature only), and lines come from the spans. Anything valid TOML
  parses, whatever its layout, and invalid TOML is refused with the parser's message at its
  line. The known tables keep their strict keys and read only strings, booleans, whole numbers
  of at least 0 and arrays of strings. Nesting is bounded by the parser.
- **Dev's gate judges every change** (CFG-001). While the integration branch has CI, every
  commit on its way there is judged by the integration branch's jobs, never its own. This covers
  a pull request's head, a merge candidate and a playground task's merge. A commit's own jobs
  run only where the branch has no gate (bootstrap, or a file never valid), and they keep the
  trusted security job.
- **A change's own file is checked as CI will read it after the merge** (CFG-002). Which file is
  checked:
  - a branch, a merge candidate or a playground merge: the commit's own;
  - a pull request: its own when it changed the file since its merge base, else the integration
    branch's.

  An invalid one fails the run at once. A job `yulius.toml` fails with the parser's message, and
  the other jobs are skipped unrun, so the change cannot merge. A pull request that does not
  repair an invalid `dev` fails naming `dev`'s problem.
- **The last valid gate runs while dev's file is invalid** (CFG-003). The gate in force is that of
  the newest first-parent commit of the integration branch, at most 100 back, whose file is
  valid. It runs until a fix merges.
- **It says so** (CFG-006). That gate's source is `last_valid_yulius_toml`. Each run judged by it
  carries a note, the CI page names the problem and the commit, and never says "CI is not
  running" then. A fix gets a run and can merge on it, so `delivery_merge` is never refused for
  want of CI. The secrets that gate's jobs were granted stay granted: the integration branch
  wrote them, as before.
- **Measured** (CFG-004). Each episode in which a sandbox's CI has no gate because of its
  configuration is audited (`ci.config_blocked`, then `ci.config_unblocked`). `GET
  /api/analytics/merge-queue` adds up its minutes (`config_blocked_minutes`).

## Alternatives

- **Keep testing a change with its own jobs (CI-110).** Rejected: a change could weaken the gate
  that judges it (drop a job, change a command to `true`), and the pull request that broke
  autoresearch passed this way.
- **Run both the integration branch's and the change's own jobs.** Rejected: a change that renames
  a script its old job runs could never merge. Running the change's jobs too also adds nothing
  dev's gate does not check after the merge.
- **Turn CI off while dev's file is invalid (the old rule).** Rejected: it blocks the repair, which
  is the very incident.
- **Fall back to a built-in default gate.** Rejected: Yulius cannot know a project's checks.
  Running none would let anything merge, and running a guess could fail every change.
- **Let a pull request that repairs dev be judged by its own file.** Rejected for a branch that had
  a valid gate: that gate is known and was in force, and the repair must not weaken it. It is
  kept for a file that was never valid, where no gate exists.
- **Remember the last valid gate in the database.** Rejected: Git already holds it, the first-parent
  walk is bounded, and it is read only while the file is invalid (cached per head for a minute).
- **Extend the hand-written reader to more TOML.** Rejected: autoresearch hit a valid layout it
  refused, and every later one would be another such incident. A maintained parser reads the
  whole format; the domain gains one small dependency.

## Consequences

- A change to `[[ci.jobs]]` is tested after its merge, by the integration branch's next run. A
  change that renames what an old job runs must keep the old command working until it has
  merged, or land in two steps.
- While `dev`'s file is invalid, every pull request that does not repair it fails its run, naming
  `dev`'s problem, and the repair passes on the last valid gate.
- A file that was never valid still leaves nothing to fall back on. The change that repairs it is
  tested with its own jobs, and those minutes are counted.
- Messages for invalid TOML are the parser's ("duplicate key", "unclosed array, expected `]`"),
  no longer Yulius's own, and some layouts the old reader refused now parse: multi-line strings,
  `4_0`, inline tables, dotted keys.
- Runs on the last valid gate have the source `last_valid_yulius_toml`, which older clients show
  as is.
