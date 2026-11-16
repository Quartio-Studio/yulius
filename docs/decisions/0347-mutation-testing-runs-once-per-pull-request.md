# 0347. Mutation testing runs once per pull request

## Status

Accepted (2026-11-16). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (MUT-001,
MUT-002, fix 8); task #472.

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
  - Each mutant is tested with nextest across the workspace. The product's test binary is
    narrowed to the tests the branch added or changed, the ones meant to hold its lines; it is
    the slowest by far, about 9 minutes whole. `--tests` gives another filter.
  - Bounds: 30 minutes for the whole run (`--minutes`) and 300 s per mutant
    (`--mutant-seconds`), with 2 jobs (`--jobs`). The run ends at the bound and names the
    mutants it did not reach.
  - The last line is the summary for the spec's Evidence. The exit code is non-zero when a
    mutant was missed, timed out or not reached.
- **Once per pull request, by the author**: on the head the review is asked for, with the
  summary in the Evidence. A project without such a command undoes by hand the few lines its new
  tests are meant to hold, and says what failed.
- **The reviewer reads the result**, may undo a single line it doubts, and runs no campaign. A
  test that cannot fail is still fixed in the same pull request (ADR-0141). The rule is stated
  the same way in the skills, the workflows and this repository's documents, and a test holds it.

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
