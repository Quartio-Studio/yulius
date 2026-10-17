---
yulius-task: a1e80fc6-d84f-4f2b-9f91-efb94e7c8716
status: in-progress
---

# SPEC-217: The Vidoc audit, medium findings in a change block, and every project's CI scans

Status: in-progress
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
  added or modified since the merge base: `git diff -U0` per file, with literal pathspecs. A new
  file is changed in full. A finding is **in the change** when its line is within 3 lines of a
  changed line (the model's line attribution is approximate). A finding with no line is in the
  change only when its file is new. Everything else is **outside the change**.
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
  path, the severity and the finding's line with its whitespace collapsed (the title when there is
  no line). Old code that shifts keeps its fingerprint. In `--ci` mode, each such finding is also
  printed as one line, `::yulius-finding <json>`, with `severity`, `file`, `line`, `title`,
  `detail` and `fingerprint`.
- **VSP-005.** The JSON report (`--report`) gives each finding `scope` (`change` or `outside`) and
  `fingerprint`.

### Issues from CI (VSI)

- **VSI-001.** When a job keyed `security` ends with `passed` or `failed` on a pull request head,
  Yulius reads its output's `::yulius-finding` lines. It reads at most 64 of them, and each must
  be a valid finding: severity medium or low, a non-empty file of at most 512 bytes, a 16-hex
  fingerprint, and a title and detail cut to 200 and 2000 bytes. Yulius files each one whose
  fingerprint the sandbox has not filed before, at most 5 per job. No other job's output is read
  for findings.
- **VSI-002. De-duplication.** Filed fingerprints are kept per sandbox (migration 0036,
  `security_findings`). A fingerprint is claimed before the issue is opened and released if
  opening fails, so concurrent runs cannot file it twice. A fingerprint whose issue was closed is
  not filed again: closing it was the decision.
- **VSI-003. The issue.**
  - Title: `security: <title> in <file>`.
  - Labels: `type: bug`, `priority: medium` or `priority: low`, `area: security` and
    `source: vidoc`.
  - Body: the finding (severity, `file:line`, detail) quoted as untrusted scanner output, the
    pull request and run that reported it, the fingerprint marker, and a `## Acceptance criteria`
    checklist, so it meets the Definition of Ready of the "Work through open issues" automation.

  The job's output gets one line for each issue filed, each one already filed, and each failure.
  A playground or a sandbox without a GitHub token files nothing, and the output says so.

### Default pipeline (VDP)

- **VDP-001.** When a sandbox's applied agent secrets include `VIDOC_API_KEY` (its own, or the
  instance's Vidoc key), the CI in force gets a built-in job, unless it already has one:
  - `name = "security"`, `command = "yulius security-scan --ci"`, `secrets = ["VIDOC_API_KEY"]`;
  - "already has one" means a job keyed `security`, or a job whose command runs `security-scan`
    or `vidoc_scan`.

  The same rule applies to the CI view (`ci_gate`) and to the verified gate the grant is read from
  (`ci_verified_gate`). A sandbox without CI, and a playground, get no built-in job. `enabled =
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

- [ ] Broad Vidoc audit of dev run and every finding triaged
- [ ] The scan fails on medium findings in the changed lines and hands low ones to the agent
- [ ] Findings outside the change are printed with fingerprints for CI to file
- [ ] Yulius CI files each outside-the-change finding once as an issue
- [ ] Every project with a Vidoc key gets the security job in its CI
- [ ] Briefing, onboarding, infra bootstrap, AGENTS.md, docs and ADR-0172 describe the policy
- [ ] Audit findings fixed, ignored with reasons, or filed
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, Evidence written, changes log updated

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
  - the built-in job appears for a sandbox with a Vidoc key.
- The store migration's test: claim and release.
- The live CI `security` job on this pull request runs the new scanner.

## Decisions

- [ADR-0172](../../docs/decisions/0172-medium-findings-in-the-change-block-and-the-rest-become-issues.md):
  medium findings in the changed lines block, low ones are the agent's call, findings outside the
  change become de-duplicated issues, and the security job is built into every project's CI when
  a Vidoc key is available.
