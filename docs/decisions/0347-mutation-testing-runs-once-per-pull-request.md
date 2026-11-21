# 0347. Mutation testing runs once per pull request

## Status

Accepted (2026-11-16). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (MUT-001,
MUT-002, fix 8); task #472. Amended (2026-11-17) by
[SPEC-506](../../.ai/specs/SPEC-506-as-fast-as-possible-from-start-to-merge.md) FST-004 and
FST-005, task #488: the run starts with the first review and runs beside it, a later round's fix
is run on its own lines, and the jobs follow the mutants listed and the budget in place of "2
jobs" below (see Amendment).

Superseded on 2026-11-21 for delivery campaigns by [ADR-0439](0439-delivery-checks-mutations-by-hand-and-builds-on-disk.md).

## Context

Review rounds blocked on test strength: a test that could not fail, an assertion too weak to
catch the line it was written for. Reviewers then ran their own mutation campaigns, one mutant at
a time, and authors answered with hand-written mutation scripts, once per finding and once per
round. The same lines were mutated again in every round, and the reading cost a round each time.
No one command existed.

## Decision

- **One command, `scripts/dev mutants`**:
  - It runs the pinned `cargo-mutants` (`.config/tools.toml`, 27.1.0) on the Rust lines the
    branch changed against the integration branch (`--in-diff`). The test tool (`tools/`) and
    test folders get no mutants.
  - Each mutant is tested with nextest across the workspace, `--all-features`, the product's
    own unit tests included. Its integration test binary, the slowest by far (about 9 minutes
    whole), is narrowed to the tests the branch added or changed (every function a hunk of
    `git diff -W` is in), the ones meant to hold its lines; `--tests` gives another filter.
  - cargo-mutants runs with no shared target directory and none of an outer nextest's
    settings: copies building into one target directory report mutants caught or missed
    wrongly.
  - Bounds: 30 minutes for the whole run (`--minutes`) and 300 s per mutant
    (`--mutant-seconds`), with 2 jobs (`--jobs`). The run ends at the bound and names the
    mutants it did not reach.
  - The last line is the summary for the spec's Evidence. The exit code is non-zero when a
    mutant was missed, timed out or not reached.
- **Once per pull request, by the author**: on the first head a review is asked for, with the
  summary in the Evidence. A project without such a command undoes by hand the few lines its new
  tests are meant to hold, and says what failed.
- **The reviewer reads the result**, may undo a single line it doubts, and runs no campaign. A
  test that cannot fail is still fixed in the same pull request (ADR-0141). The rule is stated
  the same way in the skills and this repository's documents, and a test holds it; the Feature
  and Fix workflows, whose instructions have no byte to spare, reach it through
  `delivery-pipeline`.

## Alternatives

- **The whole workspace for every mutant.** Correct and too slow: a full product test run per
  mutant, hours for an ordinary change.
- **Each mutant's own package only** (cargo-mutants' default). Here most behaviour is tested by
  the product's integration tests, so mutants in `yulius-app` would read as missed when they are
  not.
- **In Yulius CI on every head.** It would cost a campaign per push; the author's run on the
  reviewed head is enough, and CI keeps the gate's own minutes.

## Consequences

- Measured on this repository, 2026-11-16, with 2 jobs:
  - The change of fix 7 against its base gave 248 mutants (with the work merged since).
  - In the 25-minute bound, 84 were tested: 59 caught, 25 unviable, none missed. That is
    about 18 s a mutant.
  - The other 164 were named as not reached.
  - The peak memory was 5.6 GB.
  - A pull request of about 80 mutants fits the 30-minute default.
- A larger change is told which mutants were not reached. The author raises `--minutes`, or
  splits the change.
- A week later: the share of blocking review rounds whose findings are about test strength only,
  read from the rounds' reports (fix 1's records).

## Amendment (2026-11-17, SPEC-506)

- **Beside the review, not before it.** The author waited for the run before asking for the
  review, so the two were in a row: in quartio-v2 mutation runs were 33 % of task time on 17 Nov
  (a median 30 of a task's 108 minutes), here 11 % (#479: 5 runs, 135 minutes). The rule is now:
  "Mutation testing runs beside the review: when you ask for a pull request's first review, start
  the project's mutation command in the background and let the reviewer read meanwhile; when it
  ends, put its summary line in the spec's Evidence and fix each missed mutant test-first before
  the merge. After a later round's fix, run it on that fix's lines alone." The same mutants are
  tested and every missed one is fixed before the merge; what changes is that round 1's reviewer
  judges test strength by reading, without the run's result. A missed mutant's fix is a test,
  which ADR-0345 does not count, so it does not need another round; its proof is the mutation
  command run again, its summary line (0 missed) in the Evidence. A fix in product code is
  counted as any other.
- **A fix's lines alone.** `scripts/dev mutants --base <reviewed head>` mutates only what changed
  since that head (with a merge of `dev` since then, its lines too).
- **Jobs from the mutants and the budget.** With no `--jobs`, one job per 8 mutants listed, at
  least 2, at most a third of `YULIUS_CPUS` (the rest is the review's and CI's, beside it), one
  per 12 GB of `YULIUS_MEMORY_MB` and 7; 2 with no budget. The run keeps near the CPUs: the
  builds share cargo-mutants' jobserver, which gets `YULIUS_CPUS` tasks (by default the host's
  64); tests take no jobserver task (the pinned tool's book), so each job's tests get their share
  as threads. Overlapping builds and tests can pass the budget for a while: it bounds the mean. Measured on PR
  #1392's change (260 mutants, 17 Nov 2026): to the 36th mutant, 2 jobs took 866 s and 7 took
  476 s (1.8 times); to the 8th, both about 350 s, since each job first builds the workspace in a
  copy of its own (one cold build at 2 jobs, six at 7). So a fix's few mutants keep 2 jobs, and
  only a whole change gets more. The CPU of a run grows with its jobs by those cold builds; on a
  busy host the kernel's weights share it, and CI keeps four times an agent's share within the
  sandbox. The numbers are in SPEC-506's Evidence.

## Amendment (2026-11-19, SPEC-533)

The clean baseline and each mutant use Cargo's workspace selection and the same nextest filter.
In cargo-mutants 27.1.0, `--test-workspace true` applied only to mutants. The baseline omitted
tests in packages without mutations. In Git-free copies the xtask missing-key test then failed
on every viable mutant in PR #1360. Earlier summaries, including the measurements above, can
overstate catches. Inspect individual logs for the failing test or rerun the change with the
corrected baseline before relying on those counts. [Issue #1364](https://github.com/Quartio-Studio/yulius/issues/1364)
