# 0133. A CI job that checked nothing ends skipped

## Status

Accepted. Specified in [SPEC-162](../../.ai/specs/SPEC-162-all-security-checks-kept-getting-yulius.md);
task #153, issue #530. Amends [ADR-0116](0116-ci-jobs-use-named-agent-secrets-through-the-proxy.md):
the `security` job is granted `VIDOC_API_KEY`, and its exit 2 ends it skipped instead of passed.

## Context

A Yulius CI job ended passed (exit 0) or failed (any other exit). This repository's `security` job
cannot always scan: no key, a rejected key (#470) or an unbound host make `scripts/dev security`
exit 2. Failing the job then would block every pull request over something only a person can fix,
so the job turned exit 2 into exit 0 and read **Passed** although nothing was scanned. The person
asked: "If Vidoc API key is not set, default behaviour should be 'skipped' so user knows exactly".

## Decision

- **Skipped is a job state.** `RunState::Skipped` (`skipped` on the wire and in the store, a text
  column, so no migration). A job whose process exits with **78** ends skipped and keeps its exit
  code; any other non-zero exit still fails. 78 is `EX_CONFIG` from `sysexits.h`, the code Drone
  uses for "skip".
- **Yulius tells every job the code**: `YULIUS_CI_SKIP_CODE=78`. A job exits
  `"${YULIUS_CI_SKIP_CODE:-0}"` to say it checked nothing, so the same command passes on an instance
  that does not know skipped yet, instead of failing every pull request there.
- **A run is never skipped.** A run whose jobs passed or were skipped is passed and passes the
  merge gate; failed, error and cancelled still win. A result copied to another head (a merge with
  the same tree, a merge train's verdict for its pull requests) keeps each job skipped where it was
  skipped. Its commit status counts the skipped jobs
  (`Passed: 3 jobs, 1 skipped in 9 min 4 s`), the CI page shows a Skipped badge and counts them in
  the run's line, and `ci_status` shows a skipped job's output, which says why.
- **The `security` job** maps exit 2 (every "could not run" cause, not only a missing key) to the
  skip code with `[yulius] security scan skipped: …`, ends skipped on heads it does not scan (the
  integration and production branches, merge trains), and names `secrets = "VIDOC_API_KEY"`: the
  running instance knows `secrets` since `dev` was promoted to `main` with SPEC-142 (#517).

## Alternatives

- **A per-job `yulius.toml` key (`skip_exit_codes = "2"`, like GitLab's `allow_failure:
  exit_codes`).** Explicit, but the running instance is built from `main` and rejects a key it does
  not know together with the whole `yulius.toml`, which stops CI for every pull request until a
  promotion (F 2026-10-15). An environment variable needs no new key.
- **A marker line in the output (`::yulius-skip`).** A test or a tool could print it by accident and
  turn a run green; an exit code is the process's deliberate verdict.
- **Fail the job on exit 2.** Every pull request would be blocked while the key is missing or
  rejected (#470), by what only a person can fix.
- **Keep exit 0 and rely on the output.** The person reads Passed and believes the code was
  scanned; this is the defect.
- **A skipped run state.** A run of only skipped jobs would need its own gate rule and status; the
  job state and the counted description already say it, and the gate stays one rule.

## Consequences

- Every project's CI jobs can skip themselves the same way; a job that checks nothing reads
  Skipped, not Passed.
- 78 is also a real failure code of some tools (`EX_CONFIG` in sysexits-style programs, curl's
  "remote file not found"): a job whose last command fails that way now reads Skipped and blocks no
  merge. Drone accepted the same trade-off; a project whose job can end that way ends its command
  with an explicit exit (`|| exit 1`). The skip is visible on the job, in the run's line and in its
  commit status, never silent.
- Until a person promotes `dev` to `main`, the running instance gives no `YULIUS_CI_SKIP_CODE`: the
  `security` job then exits 0 and reads Passed, with `security scan skipped` in its output.
- A skipped job blocks no merge, including when a pull request itself causes the exit 2 (a
  malformed `.config/vidoc.toml`, a broken clone). Making the scan a required check once a key
  works stays the follow-up on #466; the skip is now visible on the run, its commit status and
  in `ci_status`.
- The grant is read from the integration branch, so the `security` job gets the key from the first
  run after this change is merged into `dev`.
