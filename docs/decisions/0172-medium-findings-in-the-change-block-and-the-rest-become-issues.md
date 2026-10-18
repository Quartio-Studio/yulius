# 0172. Medium findings in the change block, low ones are the agent's call, the rest become issues

## Status

Accepted. Specified in [SPEC-217](../../.ai/specs/SPEC-217-vidoc-security-didn-t-work-for-a-while-a.md);
task #160.

It amends the verdict of [ADR-0157](0157-vidoc-is-an-optional-instance-key-and-the-scan-runs-on-vidoc-1.md)
(only critical and high findings failed) and the onboarding-only security job of the same ADR. The
skip rule of [ADR-0133](0133-a-ci-job-that-checked-nothing-ends-skipped.md) and the switch of
[ADR-0107](0107-a-committed-switch-turns-the-vidoc-gate-on-or-off.md) are kept.

## Context

- **Nothing blocked medium findings.** The scan failed only on critical and high findings, and
  printed medium and low ones in the CI log, where nothing tracked them. The person asked for
  medium findings to block CI, for low ones to go to the task's agent to decide, and for this to
  hold for every project, not only Yulius.
- **The scan judges whole files.** The change scan sends every file a pull request touched, so it
  also reports old code the pull request never changed. Task #192 was flagged for a years-old
  command in `main.rs`.
- **Severity is the model's call.** Medium is its fuzziest level. Blocking every medium finding in
  a touched file would stop pull requests over code they never touched, and would teach agents to
  add ignore entries just to get through.
- **Only onboarding added the scan.** Projects onboarded before the instance had a Vidoc key had no
  scan in CI.
- **The person chose.** The proposal to block medium findings only on the changed lines, and to
  file the rest as de-duplicated issues, was accepted, with two additions:
  - it must be in the default CI pipeline and work for every project;
  - filed issues must follow the code: "fix this issue thingy and close those that are already
    closed in the scope".

## Decision

- **Verdict.**
  - Critical and high findings fail the scan anywhere in a scanned file, as before.
  - Medium findings fail it on the lines the change touched: within 3 lines of a line `HEAD` added
    or modified since the merge base (`git diff -U0`), or anywhere in a new file.
  - In the broad scan every line counts as touched, so a medium finding anywhere fails it.
- **Low findings in the change go to the agent.** The scan prints them "for you to decide": fix
  each one test-first, or say in the spec's Evidence why not. They do not fail the scan.
- **Findings outside the change become issues.** Medium and low findings outside the change get a
  fingerprint: SHA-256 of the path and the finding's line text, whitespace collapsed. A shifted
  line keeps it, and so does a finding the model words differently. With `--ci`, each one is
  printed as a `::yulius-finding <json>` line.
  - When a pull request head's `security` job ends, Yulius files each new fingerprint as an issue,
    at most 5 per job. The issue carries the Definition-of-Ready labels plus `area: security` and
    `source: vidoc`, so the "Work through open issues" automation can pick it up.
  - Yulius keeps the fingerprints it filed per sandbox (migration 0036), claimed before the issue
    is opened, so nothing is filed twice.
- **Issues follow the code.**
  - A closed issue whose finding is reported again is reopened with a comment. A false positive
    is meant to get a reviewed `[[ignore]]` entry, which stops it being reported.
  - When a `security` job of a pull request or of the integration branch ends, Yulius checks each
    unresolved filed finding against `origin/<integration branch>`. If no line of its file has its
    fingerprint any more, or the file is gone, Yulius closes the issue as completed, with a
    comment.

  Filing and reopening also look at the integration branch: a finding whose code is gone from it
  is neither filed nor reopened, because a branch behind it still reports old code. All of these
  checks are deterministic. A model that skips a finding on one run never closes its issue, and
  code that is only on a pull request branch closes nothing.
- **The job is in every project's default pipeline.** When a sandbox's applied agent secrets
  include `VIDOC_API_KEY` (its own, or the instance's Vidoc key), Yulius adds a built-in job,
  `security`: `yulius security-scan --ci`, granted `VIDOC_API_KEY`. Two conditions apply:
  - it is added only when the sandbox has CI and its CI does not already scan (no job keyed
    `security`, no job running `security-scan` or `vidoc_scan`);
  - it is added both to the CI in force and to the verified gate the grant is read from, so the
    job gets its key.

  A pull request or merge train tested with its own jobs (they changed them, or bootstrap) keeps
  the trusted security job, even past the 32-job limit, from the integration branch or built in, in place of any job it named `security`,
  so it cannot drop or fake the scan. Playgrounds get none. `enabled = false` in
  `.config/vidoc.toml` still turns the scan off.

## Alternatives

- **Block every medium finding in a touched file.** This is the simplest change. It was rejected:
  it blocks pull requests over old code and noisy model calls, and that noise breeds ignore
  entries.
- **Only print medium findings.** This was the status quo. It was rejected: a real medium bug
  merges and is forgotten.
- **Have the agent file the issues with `issue_create`.** It was rejected: nothing would
  de-duplicate the issues across tasks, and nothing files them when no agent reads the output.
- **De-duplicate by searching GitHub.** It was rejected: the search index lags, so concurrent runs
  would file duplicates, and a closed issue is not found by an open-issue search. A row per
  fingerprint is exact.
- **Close an issue when a later scan no longer reports its finding.** It was rejected: the model
  varies between runs, and a pull request branch is not the integration branch. Recomputing the
  fingerprint on the integration branch is exact.
- **Add the job only at onboarding (ADR-0157).** It was rejected: projects onboarded earlier never
  get it.

## Consequences

- A pull request must be clean at medium in the lines it touches. Old code in a touched file
  turns into issues instead of blocks. An agent sees its low findings when it runs the scan before
  delivery, and decides.
- Every project with a Vidoc key scans its pull requests without editing `yulius.toml`. A project
  with its own `security` job keeps it. This repository keeps its own job, which runs the pull
  request's scanner.
- The issues are created by the GitHub App installation. In a repository that runs its own
  scanner (this one), a pull request controls the finding lines its job prints. That gives it no
  more than an agent's `issue_create` already does, and the run is bounded: at most 64 lines read,
  5 issue filings and 10 reopen or close calls per job. The text is quoted as untrusted data. The
  scanner prints the model's titles and file names on one line each, so a prompt injection in the
  scanned code cannot forge a finding line.
- A claim is taken over after an hour without an issue. If GitHub opens an issue but its answer
  is lost, the claim is released and a later run files a duplicate. That is rare, and a person
  closes the duplicate.
- The model's line numbers are approximate. The 3-line margin trades a few old findings blocking
  for a fix-or-ignore, against real new findings slipping into issues.
- The built-in job and the issue handling reach other projects only when the instance runs this
  release (after promotion to `main`). This repository's own job uses the new verdict from the
  merge on.
