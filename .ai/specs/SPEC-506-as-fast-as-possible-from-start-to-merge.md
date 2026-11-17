---
yulius-task: cb7f5f81-48de-48ef-be3e-d04f43e32d69
status: in-progress
---

# SPEC-506: As fast as possible from start to merge, with nothing taken from quality

Status: in-progress
Date: 2026-11-17
Owner: Yulius task #488
Scope authorization: task #488, the person's words of 2026-11-17 ("make everything as efficient
as possible without quality loss, if it can be done, because I don't know"). Builds on SPEC-493
(task #472: ADR-0340 to ADR-0347), SPEC-497 (task #479: ADR-0353, ADR-0357, ADR-0358) and
SPEC-504 (task #486). The measurements are the person's (the task's request); this task cannot
read the production instance's database, transcripts or audit log.

## Goal

Find out, with the numbers, how much faster the way from a task's start to its merge can get with
nothing taken from quality; build what can be built; and bring what would cost quality, or is not
known to be free, to the person with its price instead of building it.

## Context

### Where the time goes (the request's measurements, 17 Nov 2026)

Agent working 39 %, review rounds 24 %, local tests and gate 13 %, mutation runs 13 % (quartio-v2
33 %), CI and the merge queue 10 %. A quartio-v2 task: a median 108 minutes from start to first
merge, of which the agent works 29, mutation runs 30, review 19, CI and the queue 12, local tests
10. Rounds per pull request 3.2; re-reviews after a request for changes ask again in 50 %; 20 % of
rounds re-approve an approved change with ADR-0345 in force; one more round costs a median 15 and
a mean 23 minutes.

### What the code and the rules say (read on `dev` at b1886f44d)

**C1. A reviewer asks for changes because of CI alone.** `om-code-review` makes "a failing gate"
a blocker, says "a failed job is a blocker, even if it looks unrelated", "a command that cannot
run is reported as NOT RUN … and the verdict stays request changes until it runs", and "never
approve without the head's Yulius CI run". A run still going is "reported as running" with no
verdict rule. The merge already needs a passed run on the exact head (`delivery_merge`). So a
reviewer that found nothing still blocks on a run that is unfinished or failed outside the change,
and the author's next push then needs a new round. The request read 3 of 58 repeated blocks whose
only reason was CI, and 16 more that named CI beside their finding.

**C2. The author asks again without proving the fix.** Nothing in `om-fix`, `om-code-review` or
`delivery-pipeline` asks the author, before a new round, to name the test that fails without each
fix or to check every finding of the last report against the new head. Of the 58 repeated blocks
read, 15 were "a fix or a rule with no test that can fail" and 11 "an earlier finding only partly
fixed": 26 of 58 (45 %) are what that check catches. `scripts/dev mutants --base <commit>` already
mutates only the lines changed since `<commit>` (the merge base of the reviewed head and `HEAD` is
the reviewed head), so the lines of a fix can be mutation-tested alone; no rule says so.

**C3. Mutation runs are slow and in a row with the review.** `cargo xtask mutants` defaults to
`--jobs 2` while a session has 22 CPUs; ADR-0347 measured 18 s a mutant at 2 jobs and a peak of
5.6 GB. Its rule (in `om-code-review`, `delivery-pipeline`, `test-driven-development`,
`yulius-verify`, `yulius-review`, `development.md`) says "on the first head you ask a review for,
run the project's mutation command and put its summary line in the spec's Evidence", and "the
reviewer reads that result": the author waits for the run before asking. quartio-v2's own command
is its task (#261, running).

**C4. Where re-approvals come from.** Eight messages Yulius types into a session send the author
to a new round after any fix, though ADR-0345 keeps the approval across 20 product lines:
`pr_watch.rs` (a red run: "once the new head passed and its review approved it, call
`delivery_merge` again"; a conflict), `merge_train.rs` (two dropped merge candidates: "fix it
test-first, get the review of the new head"), `agent_wait.rs` (an armed merge dropped because the
branch moved: "Get the independent review of the new head"; one dropped by failed checks; a
conflict's hold), and
`wait_cli.rs` (`yulius wait` on failed checks: "get the review of the new head"). Six of them
follow a red run or a dropped merge; the two of a conflict are right, since ADR-0345 needs a new
round after a resolved conflict. Four notices of a green head with no merge armed ask for the
review of that exact head too (`pr_watch.rs` twice, a project without CI, `yulius wait`). These are the
moments right after an approval at which a small fix is made: a red run, a failed merge
candidate. How many of the 20 % they explain is the person's number to read (Decisions).

**C5. Whole suites and the gate still run locally.** Every rule says "the whole gate is Yulius
CI's" since ADR-0342 (16 Nov). What still says otherwise: the playground's definition of done and
briefing line "run every project gate yourself" (right where CI cannot run, a playground without
CI, but also said to a playground with CI), the person's own Claude Code memory of this project
(eight entries on how to run the local gate: an exclusive target directory, no cargo beside it,
`tail --pid` on it), and summaries of older sessions carried into resumed ones. Other projects'
own rules (quartio-v2's whole web suite at `nice 19`, VistulaValley's 16 local gate runs) are
theirs.

**C6. Cheap checks fail in CI.** PR #1392's run `b9d60ec8` failed after 2 min 33 s on `cargo fmt
--check` (a one-line wrap in `leftovers.rs`), behind 68 s of Python unit tests and 76 s of `cargo
check`. Here the format check takes 8.6 s and the repository contracts 11.6 s
(`scripts/check_repository.py`, measured in this worktree). Yulius pushes a task's branch from
the server with hooks off (`core.hooksPath=/dev/null`, outside containment, #54), so a project
command cannot run at Yulius's push; the agent's own `git commit` runs hooks inside containment.
No rule names a project's fast checks as one command.

**C7. CI length here.** Recent pull-request runs: 5 to 10 minutes; `quality` (the whole gate but
the tests) is the long pole at 5 to 10 minutes, the two test shards about 4.5, e2e about 5.2. A
dev head whose tree a pull request already passed is not run again (tree reuse, in force). The
product project's 21 jobs pinned to 3 CPUs are its `yulius.toml`.

### Candidates: finding, price, expected gain

| # | Candidate | Free? | Expected per pull request | Sure? |
|---|---|---|---|---|
| 1 | No request for changes for CI alone | yes: the merge still needs green CI on the exact head; a failure the change causes stays a finding; a fix of it falls under ADR-0345 (5 % of such re-reviews asked, 23 of 457) | about 1 to 2 min (5 % of repeated blocks × 23 min, plus rounds that waited for a run) | high on the price, low on the size |
| 2 | Author proves each fix | yes: adds checks, removes none | 0.2 rounds × 23 min ≈ 4 to 5 min if half of the 45 % are caught | medium |
| 3 | Mutation beside the review, faster | yes: the same mutants are tested and every missed one is fixed before the merge; the reviewer no longer reads the result in round 1, but judges test strength itself and missed mutants are fixed anyway | the run's wall time on the first ask, less one more CI run after an approving round 1: about 25 min here (#479: 5 runs, 135 min), up to 30 in quartio-v2; a whole change's run 1.8 times as fast (measured) | high on "beside", measured on the speed-up |
| 4 | Stop needless re-approvals | yes: `delivery_merge` still counts the lines and refuses above 20 | up to 0.6 rounds × 5 to 15 min; the share these messages explain is the person's number | medium |
| 5 | Whole suites in CI only | yes | part of 13 % of task time; not measurable from here | low |
| 6 | Cheap checks before the commit | yes | one CI run (5 to 15 min) for each pull request a format or contract check would have failed (5 of 7 quartio-v2 pull requests on one afternoon) | medium |
| 7 | CI shorter by CPUs | yes where only CPUs decide; measure | quality 7 to 10 → about 5 min a run, 2 to 4 runs a pull request | medium until measured |
| 8 | Same reviewer continues | not known | rounds ask again 12 % against 33 % | replay needed: the person's |
| 9 | Smaller changes | not known | rounds 2.0 (≤ 20 lines) to 4.1 (> 300) | not known |
| 10 | Fewer defects at the first review | not known | 70 % of first reviews ask | study needed |
| 11 | Round cap, or no re-review after a small fix | costs quality: the request's replay misses 4 to 25 % of requests for changes | 6 to 43 % of rounds | the person's decision |

## Requirements

- **FST-001** (C4) Every message Yulius types into a task's session after a red run, a failed or
  dropped merge candidate, a dropped armed merge or failed checks under `yulius wait`, and every
  notice of a green head with no merge armed, says both cases: with a review that approved this
  head or an earlier one, `delivery_merge` with that approved head as `reviewed_head` (the
  approval carries across at most 20 product lines in all since it, and the call says when a new
  round is needed); with no approval yet, the review of the new head first. Only a conflict's
  messages ask for the review of the new head alone (ADR-0345). A test holds every such message
  to it.
- **FST-002** (C1) A reviewer does not ask for changes because of CI alone: when it found nothing
  to fix and the head's Yulius CI run is unfinished, or failed in a job the change does not cause,
  the verdict is approve and the report says what CI still has to do. A failure the change itself
  causes (its own test, its format, its lint, a contract its files break) stays a blocker. A
  command the reviewer itself could not run stays NOT RUN, and blocks only when nothing else can
  run it (CI cannot run). Stated once, the same in `om-code-review`, `delivery-pipeline`,
  `yulius-review`, `workflow.md` and the reviewer agents' prompt; a test holds them to it.
- **FST-003** (C2) Before asking for another round the author answers each finding of the last
  report with the test that fails without its fix (or why none can), checks every finding of that
  report against the new head, and, where the project's mutation command can take the lines of a
  fix, runs it on them (here `scripts/dev mutants --base <reviewed head>`). Stated once, the same
  in `om-fix`, `om-code-review`, `delivery-pipeline`, `yulius-review` and `workflow.md`; a test
  holds them to it.
- **FST-004** (C3) The mutation run starts when the first review is asked for and runs beside it;
  its summary goes in the Evidence when it ends and every missed mutant is fixed test-first before
  the merge. After a later round's fix only that fix's lines are run. The rule is stated once, the
  same everywhere ADR-0347's rule is, and ADR-0347 is amended.
- **FST-005** (C3) With no `--jobs`, `scripts/dev mutants` runs one job per 8 mutants listed, 2
  to 7 (the most measured), at most a third of `YULIUS_CPUS` and one per 12 GB of
  `YULIUS_MEMORY_MB`; 2 with no budget. The builds share a jobserver of `YULIUS_CPUS` tasks and each
  job's tests get their share as threads. Measured before and after on one real change, numbers in
  the Evidence.
- **FST-006** (C6) This repository has one quick command (`scripts/dev quick`: the format check
  and the repository contracts, about 20 s) and the CI `quality` job runs it first, so such a
  failure ends the run in seconds. A project names its quick command as `quick` in `yulius.toml`
  `[commands]`; the briefing and `om-check-and-commit` send the author to it before each commit
  that will be published.
- **FST-007** (numbers) The Review card and `GET /api/analytics/reviews` give, for the pull
  requests merged in the window: re-reviews after a request for changes and how many asked again,
  and rounds after an approval (re-approvals) and how many asked for changes.
- **FST-008** (C7) The `quality` job is split where only CPUs decide its length, measured before
  and after on this repository's own CI.
- **FST-009** Candidates 8 to 11, and every rule of the request's "What must stay", are not
  changed by this task; they are brought to the person with their numbers.

## Plan

- [x] Yulius's typed messages keep an approval across a fix within 20 product lines
- [x] A reviewer approves a change whose only open point is CI
- [x] The author proves each fix before asking for another round
- [x] Review of the round rules (pull request 1) approved
- [x] Mutation runs use the session's CPUs, measured on a real change
- [x] Mutation testing runs beside the first review, then on each fix's lines
- [x] Review of the mutation rules (pull request 2) approved
- [ ] One quick command runs first in CI and before each published commit
- [ ] Review of the quick checks (pull request 3) approved
- [ ] The Review card counts re-reviews that ask again and rounds that re-approve
- [ ] Review of the review numbers (pull request 4) approved
- [ ] The quality job is split where only CPUs decide its length
- [ ] Review of the CI split (pull request 5) approved
- [ ] Companion requests for quartio-v2, product and VistulaValley handed to the person
- [ ] The report and the decisions on candidates 8 to 11 brought to the person

## Acceptance criteria

- **AC-1** (FST-001) A red run, a failed merge candidate, a dropped armed merge, `yulius wait` on
  failed or passed checks and a green head with no merge armed each produce a message with both
  cases (the approved head and the 20-line bound; the review first without an approval); a test
  fails when any of them, a conflict's aside, asks only for the review of the new head.
- **AC-2** (FST-002, FST-003, FST-004) The rule sentences appear, whitespace and case aside, in
  every listed place; the old sentences ("a failed job is a blocker, even if it looks unrelated",
  "on the first head you ask a review for, run") appear nowhere; the reviewer agents' prompt says
  the CI rule.
- **AC-3** (FST-005) With `YULIUS_CPUS=22` and no `--jobs`, 8 mutants run 2 at once, 36 run 5 and
  260 run 7, within a jobserver of 22 tasks; a 4-CPU budget gets 2 and a small memory 1; `--jobs`
  still decides; with no budget it keeps 2. The measurement is in the Evidence.
- **AC-4** (FST-006) `scripts/dev quick` fails on a misformatted Rust file and on a broken
  repository contract, passes on a clean tree, and the `quality` job's first stage is it.
- **AC-5** (FST-007) Over fixture rounds the numbers count a round 2 after a round 1 that asked as
  a re-review after changes (asking again when it asked), and a round after an approving one as a
  re-approval; the card shows both.
- **AC-6** (FST-008) The split's before and after run times are in the Evidence; no CPU limit that
  protects the host is raised.

## Test plan

- Unit tests of each message text (`pr_watch.rs`, `merge_train.rs`, `agent_wait.rs`,
  `wait_cli.rs`), and one test in `skills.rs` that reads those sources and refuses the old wording.
- Rule tests in `crates/yulius-app/src/skills.rs` beside SPEC-493's (`every_place_*`).
- `tools/xtask/src/mutants.rs` unit tests for the jobs default; a timed run on a real change.
- xtask tests for `quick`; `scripts/check_repository.py` keeps passing.
- `yulius-domain::review` unit tests for the new numbers; the `analytics` integration test of
  `/api/analytics/reviews`; the Pipeline card through the mock.

## Decisions

- **Candidates 8 to 11 are not built.** 8 needs a blind replay only the person's session can run;
  9 and 10 need studies of data this task cannot read; 11 trades requests for changes for rounds
  (the request's replay: 4 to 25 % of requests missed) and is the person's.
- **The quick check runs where the agent's Git runs, not at Yulius's push.** Yulius pushes from
  the server with hooks off, outside containment (#54); running project code there would break
  that boundary. A Stop-hook gate that runs the quick command before a turn's push is possible
  inside containment but adds a hook in every session's turn end; first the named command and its
  rule, then the number of such CI failures a week later decides whether the gate is needed.
- **ADR-0362** records FST-001 to FST-003: what decides a round (the code, not CI; the count, not
  the message).
- Open: how many of the 20 % re-approving rounds followed one of the messages of C4 (asked of
  the person; FST-001 is free whatever the answer).

## Evidence

### Pull request 1 (FST-001 to FST-003)

- `scripts/dev mutants` at 1031920ed, beside review round 1 (2 jobs): `mutants: 8 listed, 7
  caught, 0 missed, 0 timed out, 1 unviable, 0 not reached (bound 30 min, took 683 s)`; 4.2 of
  22 CPUs on average (`/usr/bin/time`: 423 %), 6.2 GB peak, a 202 s baseline build.
- Review round 1 (Fable 5.2) asked for changes: one major (the red-run notice reached authors with
  no approval), five minors and two nits, all own code; each fixed in the next commit with the
  tests named there.
- The round 1 fixes' lines alone (`scripts/dev mutants --base 1031920ed`, 2 jobs): `mutants: 14
  listed, 14 caught, 0 missed, 0 timed out, 0 unviable, 0 not reached (bound 30 min, took 836 s)`.
- Review round 2 (Fable 5.2) approved b4315ea8d: one minor (two notices' produced text untested)
  and four nits, all own code, fixed after it within ADR-0345's bound (test lines, the sentence
  "a review of the head", "Unless it merged already", the spec's and the index's wording).

### Pull request 2 (FST-004, FST-005)

`scripts/dev mutants --base 87c08e62a --minutes 15` at PR #1392's head 719bb8967 (260 mutants in
its Rust lines, the same order each run, `--no-shuffle`), this sandbox, 17 Nov 2026. Seconds from
the run's start (baseline included) until the n-th mutant ended, read from each run's
`debug.log` (`lab.rs: outcome` lines); CPU from `/usr/bin/time -v`; load from `uptime`:

| Run | Load (1 min) | 8th | 14th | 25th | 36th | 80th | CPUs used | Jobserver tasks |
|---|---|---|---|---|---|---|---|---|
| `--jobs 2` | 196 to 343 (busy) | 490 | 571 | 821 | — | — | 4.6 | 64 (the host's) |
| `--jobs 2` (review round 1's reading) | 63 to 148 | 358 | 456 | 694 | 866 | — | 5.1 | 64 |
| `--jobs 7` (review round 1's reading) | 82 to 148 | 342 | 368 | 431 | 476 | — | 17.2 | 64 |
| `--jobs 8` | 71 to 196 | 429 | 462 | 532 | 577 | 709 | 17.9 | 64 |
| default (`YULIUS_CPUS=22`: 7 jobs within 22 CPUs) | 37 to 52 | 279 | 303 | 363 | 409 | 540 | 17.1 | 22 |

- On the same mutants, 7 jobs are 1.8 times as fast as 2 at the 36th and about as fast at the 8th:
  each job first builds the workspace in a copy of its own (one cold build at 2 jobs, six at 7),
  which pays only over several mutants. So the default is one job per 8 mutants listed (2 for a
  fix's few, 7 for a whole change), within a third of the CPUs. My first reading ("four times",
  "adds no work") counted unviable mutants (about 8 s each, 43 % at 7 jobs against 22 % at 2) and
  was wrong: more jobs spend more CPU, by their cold builds.
- The run now keeps near the budget's CPUs: the builds share cargo-mutants' jobserver, which had
  the host's 64 tasks and now gets `YULIUS_CPUS` (22); tests take no jobserver task, so each job's
  tests get their share as threads. With the caps, 7 jobs used 17.1 CPUs on average; overlapping
  builds and tests can pass 22 for a while. On a busy host the kernel's weights share the rest
  (within a sandbox: agents 100, CI 400, engine 100); the busy 2-job run took 37 % longer to its
  8th mutant and 18 % to its 25th than the same run at a lighter load.
- A job's copy of the workspace and its build are 12 GB here (`du -sh target/debug`: 12G), in the
  temporary directory, which is memory (tmpfs): hence one job per 12 GB of the budget.
- The baseline's tests run alone with a job's thread share (the capped run's 8.5 s against 2.7 to
  5.3 s uncapped): small here, not measured for a change with many product integration tests.
- This repository's mutation runs start no container, so they never count as CI use for CI's
  admission (ADR-0353 counts a sandbox's CI groups and its Docker engine). A project whose
  mutation run starts browsers in its engine (quartio-v2) is counted; nothing caps the engine's
  CPU but the sandbox's `cpu.max` and its weight (`crates/yulius-sandbox/src/cgroup.rs`).
- No run missed a mutant or timed out; the 141 mutants common to the 7- and 8-job runs and the 36
  common with 2 jobs had the same outcome in each (review round 1's check), so load made no
  mutant read as caught.
- PR 1's own run at 2 jobs: 8 mutants in 683 s; its fix's 14 in 836 s. Pull request 2's own run:
  "No mutant in the Rust lines changed" (`tools/**` is not mutated), so its lines were undone by
  hand: the budget's variable name, the per-mutant bound and the thread share, each failing
  `the_jobs_follow_the_mutants_and_the_budget`.
- Expected gain: the run no longer comes before the review (it was a median 30 of a quartio-v2
  task's 108 minutes, 11 % of this repository's task time); an approving round 1 is followed by
  one more CI run for the Evidence line and any missed mutant's test.
- `cargo nextest run -p xtask`: 15 passed; `cargo clippy -p xtask --all-targets -- -D warnings`:
  clean; `cargo nextest run -p yulius-app --lib -E 'test(every_place_) | test(no_typed)'`: 9
  passed; `every_place_says_mutation_testing_runs_beside_the_review` fails with the old wording in
  one place (checked by hand).
- Review: round 1 (Fable 5.2) asked for changes (two majors: the jobs were not bounded by the
  budget, and the default spent CPU for nothing on small runs while the Evidence said it was free;
  five minors), fixed in 864f757b7; round 2 (Fable 5.2) approved 864f757b7 with five minors and
  four nits (test strength of the jobs rule, the caps through the real tool, the jobserver's
  wording, one percentage, FST-005 and AC-3), fixed after it in test, document and comment lines.

