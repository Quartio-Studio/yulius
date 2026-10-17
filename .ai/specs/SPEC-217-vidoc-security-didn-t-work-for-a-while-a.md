---
yulius-task: a1e80fc6-d84f-4f2b-9f91-efb94e7c8716
status: done
---

# SPEC-217: The Vidoc audit, medium findings in a change block, and every project's CI scans

Status: done
Date: 2026-10-17
Owner: Yulius task #160
Scope authorization: task #160 (requested by MrFishPL): "execute broad audit of security using
Vidoc … fix things that are messed up", "medium risks in vidoc should also be taken seriously and
block CI. Low should be passed to the task executor and it should decide. And it's should be the
behaviour in general, not only in yulius sandbox", and, on the proposal to block medium on the
lines a pull request changed and file the rest as de-duplicated issues: "this is better actually
but make sure it's in the default ci pipeline and will generalise for every other project".

## Goal

1. **Audit.** Run the broad Vidoc scan of the whole repository once, because many pull requests
   merged while the scan could not run. Triage every finding: fix the real ones, record the false
   positives, and file what lies outside this change.
2. **A stricter gate, for every project.** These rules hold for the scan Yulius runs for any
   project (`yulius security-scan`), and for this repository's `scripts/dev security`:
   - critical and high findings fail the scan anywhere in a scanned file, as before;
   - medium findings on the lines the change touched fail it too;
   - low findings on those lines go to the task's agent, which decides;
   - medium and low findings in code the change did not touch become GitHub issues, each filed
     once.
3. **In the default pipeline.** Every project whose sandbox can reach Vidoc runs the scan in its
   CI, without its own `yulius.toml` naming it.

## Context

- **Why PRs passed unchecked.** Between PR #600 (2026-10-16 ~11:50Z) and the morning of
  2026-10-17, Vidoc refused the project's `VIDOC_API_KEY` (HTTP 401, #606). The scan exits 2 ("not
  run") and the CI `security` job ends skipped (ADR-0133). A skipped job blocks no merge, so every
  pull request merged in that window had no security review. Before #600, the scan used the dead
  CLI API (`api.vidoc.dev`, #470), so no pull request was ever scanned before 2026-10-16.
  Transcripts: tasks #163, #166, #167, #172, #176, #179, #186 and #187 recorded "security unrun
  (401)". The key was replaced by 2026-10-17 midday: task #192's local scan really ran.
- **The scanner** (`scripts/vidoc_scan.py`, SPEC-195 VSC-001..012) is embedded in the binary as
  `yulius security-scan`. It fails (exit 1) only on open critical or high findings (`FAILING`), and
  prints medium and low findings. It knows the changed files, not the changed lines.
- **The noise problem** (the proposal the person accepted). The change scan sends whole files, so
  it also reports old code in a touched file. Task #192 was flagged for a years-old command in
  `main.rs`. Severity is the model's call, and medium is the fuzziest level. Blocking every medium
  in a touched file would stop pull requests over code they never touched.
- **CI for other projects.** Only the onboarding prompt adds the `security` job, and only when the
  instance has a Vidoc key (SPEC-195 VIB-002). Projects onboarded before that, or before the key
  existed, have no scan in CI. The jobs come from the integration branch's `yulius.toml`
  (`ci_jobs_at`). The grant of agent secrets is read again from the verified integration head
  (`ci_verified_gate`, review B1 of SPEC-142).
- **Issues.** `issue_create` (agents, DoR labels) opens issues through the GitHub port. Nothing
  records which findings were filed.

## Requirements

### Scan policy (VSP)

- **VSP-001. Changed lines.** The change scan works out, for each sent file, the lines `HEAD`
  added or modified since the merge base: `git diff -U0 --text` per file, from the repository's
  top, with literal pathspecs. A moved file (`-M`) is compared with its old path, and any other
  new file is changed in full. A finding is **in the change** in three cases:
  - its line is within 3 lines of a changed line (the model's line attribution is approximate);
  - it has no line and its file is new;
  - it names a file that was not sent: the model got the name wrong, so it fails closed.

  Everything else is **outside the change**. The model's titles and file names are printed on one
  line each (whitespace collapsed, control characters dropped), so an answer cannot print a
  `::yulius-finding` line of its own.
- **VSP-002. Verdict.**
  - Exit 1 on any open critical or high finding, wherever it is, as before.
  - Exit 1 on any open medium finding in the change.
  - The broad scan (`--all`) treats every line as in the change.

  Ignored findings (`[[ignore]]`) never fail.
- **VSP-003. Low findings in the change go to the agent.** They are printed under "for you to
  decide", which says what to do: fix each one test-first, or say in the spec's Evidence why not.
  They do not fail the scan.
- **VSP-004. Findings outside the change become issues.** Medium and low findings outside the
  change are printed under "outside this change". They do not fail the scan, and Yulius CI files
  them as issues. Each one has a fingerprint: the first 16 hex characters of SHA-256 over the
  path and the text of the finding's line with its whitespace collapsed (the lowercased title when
  there is no line or it is blank). Old code that shifts keeps its fingerprint, and so does a
  finding whose severity or title the model words differently on another run. In `--ci` mode,
  each such finding is also printed as one line, `::yulius-finding <json>`, with `severity`,
  `file`, `line`, `title`, `detail`, `fingerprint` and `anchored`.
- **VSP-005.** The JSON report (`--report`) gives each finding `scope` (`change` or `outside`) and
  `fingerprint`.

### Issues from CI (VSI)

- **VSI-001.** When a job keyed `security` ends (passed, failed or skipped) on a pull request head,
  Yulius reads its output's `::yulius-finding` lines. It reads at most 64 of them, and each must
  be a valid finding:
  - severity medium or low;
  - a file of at most 512 bytes that is a repository path: relative and `/`-separated, with no
    empty, `.`, `..` or `.git` segment, no leading `-` and no `\`;
  - a 16-hex fingerprint;
  - a title and detail, cut to 200 and 2000 bytes.

  Yulius files each one whose fingerprint the sandbox has not filed before, at most 5 attempts per
  job, failed ones included. No other job's output is read for findings. A finding whose code is no
  longer on `origin/<integration branch>` is neither filed nor reopened, because a branch behind it
  still reports code that is already gone; the output says how many were left out that way. A file
  name with a backtick or a control character is refused.
- **VSI-002. De-duplication.** Filed fingerprints are kept per sandbox (migration 0036,
  `security_findings`). A fingerprint is claimed before the issue is opened and released if
  opening fails, so concurrent runs cannot file it twice. The row also keeps the finding's file,
  whether its fingerprint names a line's text (`anchored`), and when Yulius resolved it. A claim
  that got no issue within an hour (its process stopped) is taken over. Known limit: if GitHub
  opened an issue but its answer was lost, the claim is released, and a later run files a
  duplicate.
- **VSI-003. The issue.**
  - Title: `security: <title> in <file>`.
  - Labels: `type: bug`, `priority: medium` or `priority: low`, `area: security` and
    `source: vidoc`.
  - Body: the finding (severity, `file:line`, detail) quoted as untrusted scanner output, the
    pull request and run that reported it, the fingerprint marker, and a `## Acceptance criteria`
    checklist, so it meets the Definition of Ready of the "Work through open issues" automation.

  The job's output gets one line for each issue filed, each one already filed, and each failure.
  A playground or a sandbox without a GitHub token files nothing, and the output says so.

- **VSI-004. Issues follow the code** (the person's follow-up, 2026-10-17: "fix this issue
  thingy and close those that are already closed in the scope").
  - **Reopened while reported.** A pull request head reports a fingerprint filed before whose
    issue is closed: Yulius reopens the issue with a comment and clears its resolution. A
    false positive is meant to get a reviewed `[[ignore]]` entry, which stops it being reported.
    Together with closing, at most 10 issue state calls are made per job, failed ones included.
  - **Closed once the code is gone.** When a `security` job of a pull request head or of the
    integration branch ends (passed, failed or skipped), Yulius looks at the unresolved filed
    findings that the job did not report while their code is on the integration branch: a random
    sample of at most 200, in at most 50 files. For each, it reads the file on
    `origin/<integration branch>`. If the file is gone, or if the finding is
    anchored and no line of the file has its fingerprint any more, Yulius closes the issue
    (completed), comments why, and marks the finding resolved. An issue GitHub no longer has
    (404, or 410 when deleted) is marked resolved too. A finding reported again whose issue
    GitHub no longer has is forgotten (only the record of that issue, so a concurrent run's newer
    one is kept) and filed anew.
  - Recomputing the fingerprint is exact (the same SHA-256 as the scanner, asserted on both sides
    by one constant). So a model that skips a finding on one run never closes an issue, and code
    still on a pull request branch closes nothing.

### Default pipeline (VDP)

- **VDP-001.** When a sandbox's applied agent secrets include `VIDOC_API_KEY` (its own, or the
  instance's Vidoc key), the CI in force gets a built-in job, unless it already has one:
  - `name = "security"`, `command = "yulius security-scan --ci"`, `secrets = ["VIDOC_API_KEY"]`;
  - "already has one" means a job keyed `security`, or a job whose command runs `security-scan`
    or `vidoc_scan`.

  The same rule applies to the CI view (`ci_gate`) and to the verified gate the grant is read from
  (`ci_verified_gate`). A pull request tested with its own jobs (it changed them, or bootstrap)
  keeps the trusted security job(s): the integration branch's scanning jobs, else, when the key
  applies, the built-in one. Each replaces the pull request's job of the same key, so a job it
  named `security`, or one that only mentions the scanner, never stands in for the scan
  (`keep_security_jobs`). A sandbox without CI, and a playground, get no built-in job. `enabled =
  false` in `.config/vidoc.toml` keeps turning the scan off (the job ends skipped).
- **VDP-002.** The briefing's "Security scan" section, the onboarding text, the infra-bootstrap
  workflow and skill, and AGENTS.md describe the policy:
  - medium findings in the change block;
  - low findings in the change are the agent's call (fix, or say why not);
  - findings outside the change become issues;
  - the CI job is built in.

### Audit (VSA)

- **VSA-001.** The broad scan of `dev` at 7b7ca3f3 runs to the end. Its report is kept under
  ignored `.ai/runs/`. Each critical, high and medium finding is triaged and fixed, recorded as a
  reviewed `[[ignore]]` false positive, or filed as an issue with the reason it lies outside this
  change. The Evidence lists them all.

## Plan

- [x] Broad Vidoc audit of dev run and every finding triaged
- [x] The scan fails on medium findings in the changed lines and hands low ones to the agent
- [x] Findings outside the change are printed with fingerprints for CI to file
- [x] Yulius CI files each outside-the-change finding once as an issue
- [x] Every project with a Vidoc key gets the security job in its CI
- [x] Filed issues follow the code: reopened while reported, closed once the line is gone from dev
- [x] Briefing, onboarding, infra bootstrap, AGENTS.md, docs and ADR-0172 describe the policy
- [x] Audit findings fixed, ignored with reasons, or filed
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, Evidence written, changes log updated

## Acceptance criteria

- Against a fake Vidoc:
  - a medium finding on a line the branch added fails the change scan;
  - a medium finding on an untouched line of a touched file does not fail it, and is printed with
    its fingerprint under "outside this change";
  - a low finding on a changed line is printed under "for you to decide" and passes;
  - critical and high findings fail the scan anywhere, as before;
  - in `--ci` mode only outside findings are printed as `::yulius-finding` lines.
- A `security` job whose output holds finding lines files one issue per new fingerprint, with DoR
  labels. A second run with the same fingerprints files nothing. Another job's lines file nothing.
- A filed finding's closed issue is reopened when it is reported again. An issue whose file is
  gone from the integration branch is closed with a comment. An issue whose line is still there
  stays open.
- A sandbox with the instance's Vidoc key and a project gate without a security job runs a
  `security` job in CI and is granted `VIDOC_API_KEY`. A project that has its own job is
  unchanged.
- The audit's triage is in the Evidence.

## Test plan

- `scripts/test_vidoc_scan.py` (unittest, real Git repositories, fake Vidoc):
  - changed lines, the tolerance and new files;
  - the medium, low and outside verdicts;
  - fingerprints that are stable across a line shift;
  - the `::yulius-finding` lines in `--ci` mode;
  - the report fields.
- `yulius-domain` unit tests: the built-in job is added once, and not when the CI already has a
  `security` job or runs the scanner.
- `yulius-app` unit tests: parsing `::yulius-finding` lines (limits, invalid lines).
- `yulius` integration tests (`local_ci`), with the fake GitHub:
  - a `security` job whose command prints finding lines files issues once;
  - a second run files nothing;
  - another job's lines are ignored;
  - the built-in job appears for a sandbox with a Vidoc key;
  - a closed issue is reopened while reported, and an issue whose code is gone is closed.
- `yulius-github` client test against the fake GitHub: reopening and closing with a comment, a
  no-op when already in that state, and 404.
- The store migration's test: claim and release.
- The live CI `security` job on this pull request runs the new scanner.

## Evidence

### The audit (VSA-001)

The broad scan of `dev` at 7b7ca3f3 ran from a detached worktree with this branch's scanner. The
command was `vidoc_scan.py --all --jobs 4 --deadline-minutes 400`, and the report is in the ignored
`.ai/runs/vidoc-broad-7b7ca3f3.json`.

- **Coverage:** 1626 files tracked. 808 were sent in 269 chunks (16 MB): 269 of 269 reviewed and
  none unscanned. 818 were skipped as prose, vendored, media or lockfiles.
- **Outages:** a first attempt at 12 requests at a time got HTTP 500 from `/v1/messages`, for
  every request, from about 19:55Z to 20:40Z. `/v1/models` still answered 200, so the cause was
  on Vidoc's side. That was reported with `needs_you` and resolved when Vidoc recovered.
- **Findings:** 10 high (1 already ignored, #626), 38 medium, 43 low. Three read-only triage
  sessions checked every high and medium finding against the code.

| Outcome | Findings |
| --- | --- |
| Fixed here, test-first | high `auth.rs:532`, a reused GitHub login takes over a linked account: #653 (`http_auth::a_reused_github_login_does_not_take_over_a_linked_account`, which fails without the fix) |
| Fixed here, test-first | medium (triaged high) `files.rs:384`, the editor's save writes through a planted temporary link: #655 (`files::a_save_never_writes_through_a_planted_temporary_link`) |
| Filed, chained task #204 | high `mcp_config/project.rs:116` plus the stdio Test: agent-imported `.mcp.json` commands run on the host: #654 |
| Filed, chained task #205 | medium (triaged high) `delivery.rs:26`, `:4095`: environment runs check out and execute uncontained on the host: #656 |
| Filed, chained task #206 | medium: imported transcripts `imports.rs:1812`, uploads `uploads.rs:283`, skill copies (destination side of `skills.rs:227`), editor read `files.rs:294`; host writes follow links: #657 |
| Tracked already | `yulius-browser/src/lib.rs:18` (#626), `claude_config.rs:174` (#634) |
| False positive, `[[ignore]]` with the guard | high: `mcp_config.rs:566`, `:576`, `playground/onboard.rs:232`, `:254`, `session_docs.rs:936`, `credential.rs:353`, `yulius-update.service:15`. Medium: `accounts.rs:1086`, `auth.rs:562`, `ci/cd.rs:1176`, `:1515`, `delivery/runner.rs:225`, `disk.rs:22`, `manage.rs:244`, `mcp.rs:86`, `mcp_config.rs:250`, `project.rs:128`, `purge.rs:59`, `preview.rs:93`, `session_docs.rs:1124`, `terminal.rs:151`, `vault/runtime.rs:830`, `yulius-domain` `disk.rs:147`, `vault/mod.rs:182`, `:251`, `proxy.rs:351`, `hook_client.rs:316`, `yulius-import` `upload.rs:162`, `taskview.js:340`, `sessions.rs:40`, `leftovers_cli.rs:299`, `update_helper.rs:338`, `wiring/git.rs:263`, `:509`, `wiring/harness.rs:58`, `:206` |

- **`.config/vidoc.toml`:** 45 of its 56 new `[[ignore]]` entries come from the audit; the other 11
  come from this branch's own change scans, below. Each one names the guard that stops its
  finding (and its test where one exists), or the issue tracking it. Applied to the audit's
  report, they leave open only the two findings fixed here.
- **Low findings (43): my call as the executor, not acted on in this change.** They are
  hardening, test fixtures (`tools/fake-agent`, `fake/synthetic_app_key.pem`) or scripts. A
  pull request that touches their code gets each one filed as an issue by the new CI path.
  - `scripts/vidoc_scan.py:437`: a finding naming a file outside its chunk. This is now covered:
    such a finding counts as in the change (VSP-001).

### Commands and results

- `cd scripts && python3 -m unittest test_vidoc_scan`: 63 tests passed. They include:
  - 20 policy tests, each failing before its fix: changed lines, slack, deletions, new and moved
    files, `-diff`, a subdirectory, unusual names, a misnamed file, forged lines, fingerprints and
    Python–Rust parity.
- `cargo nextest` on the new tests:
  - `local_ci::` (34): filing once, reopening and closing, a stale branch, a refused filing, the
    built-in job;
  - `ci::findings::` unit tests;
  - the store's `security_findings_are_claimed_once_per_sandbox`;
  - `yulius-github` `issues_are_reopened_and_closed_with_a_comment`;
  - the domain's `the_built_in_security_job_joins_a_gate_that_does_not_scan`;
  - the briefing and onboarding tests.

  All passed. The order-dependent tests were run three times each.
- `scripts/dev ci`: passed at be37290e (2206 tests), 56066584 (2207) and 7a33f248 (2209), each
  with 18 skipped, plus lint, macOS, doctests, deps and docs. The final
  head's gate: at 6023905c (code as of 544cb5b7, the last code change), it passed with 2213
  tests (18 skipped).
- `python3 scripts/check_repository.py`: passed.
- `python3 scripts/vidoc_scan.py --base origin/dev`, the change scan under the new policy, 29
  files sent in 23 chunks each time. The model's answers vary between runs, so each run was
  triaged:
  - **At 5232bfc6:** exit 0. Three low findings outside the change, which CI files as issues.
  - **At 7a33f248:** exit 1, on a high at `ci/cd.rs:1071` ("No process isolation when
    containment is disabled"). That is by design: an uncontained instance is the operator's
    documented choice. The run also reported 3 medium and 5 low findings outside the change. The
    medium ones were false positives, and the low ones were fixtures, internal-only code, or
    tracked in #657. All were recorded as `[[ignore]]` entries (9).
  - **At 4c0356ea:** exit 1, on a medium **in the change**: `ci/findings.rs:191`, a finding's file
    went to `git show` unchecked. The new policy blocked its own author. It was fixed test-first
    in 2a0b721f: only repository-relative paths are read. Two outside false positives were
    recorded.
  - **At 2a0b721f:** exit 0. Nothing fails, and nothing is low in the change. Three findings
    outside the change (a medium at `ports/github.rs:339`, lows at `auth.rs:146` and
    `ci/cd.rs:1289`) are left for CI to file as issues.
- Independent review round 1 (fresh session, at d0e9cec3) asked for changes:
  - 3 major findings: M1 subdirectory, M2 stale branches, M3 Evidence;
  - 9 minor findings and 4 nits.

  All were fixed test-first in 126712a2, 331c1c3a and be37290e, and M3 by this section.
- Independent review round 2 (fresh session, at 56066584) asked for changes, with every round-1
  fix verified. It raised:
  - the editor save's mode set by path;
  - no test of the filing cap or the 404 path;
  - one ignore entry that was too broad;
  - Evidence wording;
  - nits: 410 Gone, the close-comment wording, findings filed before past the cap, long lines,
    and a misplaced doc comment.

  These were fixed in 7a33f248, with tests `a_job_tries_at_most_five_filings_failed_ones_included`
  and `a_finding_whose_issue_is_gone_from_github_is_resolved`. The latter fails when the 404 arm
  is removed. The 410 case is covered by the `yulius-github` client test, and the mode by
  `files::a_save_never_writes_through_a_planted_temporary_link`. The pre-existing write-side race
  in `file_write` was added to #657. The swap race on the temporary file's mode is closed by
  design: the mode is set on the handle, and there is no path-based call left. It cannot be
  reproduced deterministically without a test hook in production code.
- Independent review round 3 (fresh session, at 30e7708d) verified the round-2 fixes and agreed
  with each of the 11 new `[[ignore]]` entries. It asked for:
  - a test of the reopen past the filing cap,
    `a_closed_finding_is_reopened_past_the_filing_cap`;
  - a finding whose issue was deleted to be filed again instead of left stuck,
    `a_reported_finding_whose_issue_was_deleted_is_filed_again` (it failed first), with the
    store's `forget_security_finding`;
  - the VSI-001 path rule in this spec, and line lengths.

  These were fixed in a406a6c1 and 7837d6c4. The change scan at 7837d6c4 exited 0, with one **low
  finding in the change** for the executor to decide: `repository_path` refused `.git` only as the
  first segment. I fixed it test-first (a `sub/.git/config` case). The fake-GitHub fixture's ignore
  entry was widened to `credentials`, because Vidoc worded its title differently on that run. The
  change scan at e93af855 exited 0: nothing fails, no low finding in the change, and one low finding
  outside it (`briefing.rs:877`) for CI to file.
- Independent review round 4 (fresh session, at 27aa73cc) verified the round-3 fixes. It found
  one race in my code: two concurrent runs seeing a deleted issue could each forget the finding,
  and one run's forget deleted the other's new record, filing the finding twice. Now a run forgets
  only the record of the issue it saw gone (`AND issue_number = ?3`). The store test
  `security_findings_are_claimed_once_per_sandbox` was extended first: a stale forget keeps the
  newer #43. The note now says only that the issue is gone, the migration comment mentions the
  forget, and the long line was rewrapped.
- The PR's CI `security` job (the new scanner, at 9604a467) blocked the merge on a medium in the
  change: the built-in job's detection could be spoofed. Tracing it showed a real bypass. A pull
  request that changes the CI jobs is tested with its own jobs, which replaced the gate together
  with the built-in security job, and its own job named `security` was even granted the key's
  placeholder. Fixed test-first:
  `local_ci::a_pull_request_with_its_own_jobs_cannot_drop_the_security_job` failed (the fake job
  ran), and passes with `keep_security_jobs`, which also has a domain test.
- The change scan at 746369c7 exited 0, after two more entries were recorded. One is the write
  side of the editor's path checks, `files.rs:409`, pre-existing and tracked in #657. The other
  is the placeholder-only Vidoc key grant, `onboarding.rs:138`, by design (ADR-0116). Both are
  near lines this branch changed, so they counted as in the change.

### Not tested here

- A live Yulius CI run of the built-in job on another project. The instance runs `main`, so the
  built-in job and the issue handling reach other projects only after promotion. This
  repository's own `security` job runs the pull request's scanner and uses the new verdict from
  this pull request on.
- Real GitHub issue creation and reopening. These were tested against the in-process fakes (the
  app port fake and the `yulius-github` HTTP fake).

## Decisions

- [ADR-0172](../../docs/decisions/0172-medium-findings-in-the-change-block-and-the-rest-become-issues.md):
  medium findings in the changed lines block, low ones are the agent's call, findings outside the
  change become de-duplicated issues, and the security job is built into every project's CI when
  a Vidoc key is available.
