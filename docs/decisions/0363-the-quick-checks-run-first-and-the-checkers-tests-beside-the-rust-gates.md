# 0363. The quick checks run first, and the checker's tests beside the Rust gates

## Status

Accepted (2026-11-17). Specified in
[SPEC-506](../../.ai/specs/SPEC-506-as-fast-as-possible-from-start-to-merge.md) (FST-006, FST-008);
task #488. Builds on [ADR-0342](0342-yulius-ci-runs-the-whole-gate-while-the-review-reads.md) (the
whole gate is Yulius CI's) and [ADR-0358](0358-tests-run-in-shards-and-a-run-starts-from-the-build-it-builds-on.md)
(the `quality` job runs the gate but its tests).

## Context

- A check of seconds that fails in CI costs a whole run. PR #1392's run `b9d60ec8` failed on
  `cargo fmt --check` after 2 min 33 s, behind 68 s of Python tests and 76 s of `cargo check`. On
  17 Nov, 5 of 7 quartio-v2 pull requests had a failed run on their own change: a format check, a
  repository-contract test, a palette test. Here the format check takes 8.6 s and the repository
  contracts 11.6 s.
- Yulius pushes a task's branch from the server with hooks off, outside containment (#54): a
  project's command cannot run at that push.
- The `quality` job's stages at CI's 11 CPUs, after a one-line change: `check` 88 s (11 s
  contracts, 68 s Python tests, 9 s `cargo check`), `lint` 23, `macos` 19, `docs` 7; 138 s in all.
  The Python tests take 3.4 CPU-seconds of their 68 s: they wait on local servers and timers.
- The person, 17 Nov 18:14 UTC: the host was at 99 % (CPU pressure 38 %). What spends more CPU at
  once only moves waiting on a busy host.

## Decision

- **`scripts/dev quick`**: the repository contracts and `cargo fmt --check`, 16 s on a clean tree.
  `ci` and `checks` (the `quality` job) run it right after `doctor`, and `check` and `lint` then
  leave out what it ran.
- **The rule, for every project** (`om-check-and-commit`, and this repository's AGENTS.md with its
  command): "Before each commit you publish, run the project's quick checks: its format check and
  the tests that read the whole repository (contracts, generated lists, palettes), which a change
  touches whenever it changes a file they read. They take seconds, and each one that fails in CI
  costs a whole run." `infra-bootstrap` asks a project's gate to run them first and name them in
  AGENTS.md.
- **The checker's tests run beside the Rust gates.** Once `doctor` and `quick` passed, the Python
  tests start in a thread of their own and every other gate runs meanwhile; the job ends with both
  results. Their output is printed in one piece at their end. The standalone `check` runs them
  beside `cargo check`. No job, checkout or build is added, and no CPU limit is raised.

## Alternatives

- **A `[commands] quick` key in `yulius.toml`, named by the briefing.** The running instance's
  parser refuses an unknown key (ADR-0360), so this repository's own CI would fail until the
  instance runs the new parser; a later change can add it.
- **A hook that runs the quick check before Yulius pushes.** Yulius pushes outside containment
  with hooks off (#54); a Stop hook inside the session would run it at every turn's end that
  pushes. The rule and the named command come first; the share of CI runs that still fail on a
  check of seconds decides whether such a hook is worth it.
- **Split `quality` into two jobs.** It adds a checkout, a bootstrap and a build of the host
  crates, more CPU on a busy host, for a run the test shards and e2e (4.5 and 5.2 minutes) already
  bound.

## Consequences

- A format or contract failure ends the `quality` job in seconds after its bootstrap, not after the
  build.
- At CI's 11 CPUs after a one-line change, `checks` took 108 and 114 s (load 38 to 158) against
  138 s before; the stages after `quick` no longer wait for the Python tests.
- A failing Python test is reported at the job's end, after the Rust gates, not at the start of
  `check`.
- A week later: the share of failed `quality` jobs whose first failed stage is `quick`, and the
  `quality` stage times, on the Dashboard's Pipeline tab.
