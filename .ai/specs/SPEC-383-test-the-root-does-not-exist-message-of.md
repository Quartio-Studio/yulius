---
yulius-task: 5c1c3de0-e489-454c-af19-e793384e4f3f
status: done
---

# SPEC-383: A test asserts "your home does not exist on this machine" (#643)

Status: done
Date: 2026-11-02
Owner: Yulius task #365
Scope authorization: the open-issues automation (task #365): solve GitHub issue #643 in one pull
request into `dev` saying `Fixes #643`. Create no new GitHub issue; comment on #643 instead.

## Goal

An integration test covers `secret_create_from_file` naming `~/key` in a sandbox whose `home` was
removed: the call is refused with "your home does not exist on this machine" and no agent secret
is created. Test only; the behavior is unchanged.

Added on the way (it failed every pull request's e2e job on 2026-11-02, this one's included): the
Pipeline's daily chart keeps its hover tip inside the plot when the hovered day's p90 is near the
chart's top, and its e2e test no longer depends on the day it runs.

## Context

- `open_folder` (`crates/yulius-app/src/vault/env_files.rs`, SPEC-211, task #198) walks from the
  anchor down the root's folders without links. Opening the root itself (no visible folder) maps
  `ENOENT` to `AppError::Invalid("{within} does not exist on this machine")` and `ELOOP`/`ENOTDIR`
  to "{within} is a link or not a folder: Yulius follows no links".
- A task's `~/…` anchors on the canonical sandbox folder with the folders `["home"]`
  (`secret_source_root` in `crates/yulius-app/src/vault/create.rs`); `within` is "your home". A
  missing home therefore reaches the `ENOENT` root branch of `open_folder`, not the
  `canonicalize` refusal of `secret_source_root` (that one is for a missing sandbox folder).
- Only the link branch is asserted
  (`secrets_create::a_root_swapped_for_a_link_and_reserved_names_are_refused`); the final review
  of #198 left the missing-root branch untested at the round limit (#643).
- **The e2e failure met on delivery.** This PR's first CI run (f6d16986) and #1011's (7d776c09)
  failed one e2e test, `pipeline.e2e.ts` › "the daily chart's hover tip stays inside its chart on
  the first and the last day": `pastPlot: 46` at 1280 px over "Mon, Nov 2". Reproduced locally
  every run. A probe showed the tip inside the plot horizontally (right edge 1227 = plot's right)
  but its top at 665 against the plot's 711: `placeTip` (`web/assets/stages.js`) clamps x only,
  and the tip is drawn above the point (`translate(-50%, calc(-100% - 8px))`) at the day's p90.
  The mock (`stages/summary` in `mock.js`) ends nothing on weekends and gives each weekday its own
  p50/p90, over the 30 days to today; the test came with #986 (2026-10-31, a Saturday), so its
  last day was a weekend (tip at the bottom) on every run until this Monday, whose p90 is near the
  chart's top.

## Requirements

- **RDE-001** A test in `crates/yulius/tests/secrets_create.rs` removes the sandbox home of task
  t1 and calls `secret_create_from_file {"name": …, "file": "~/key"}` with the task's token.
- **RDE-002** It asserts the call is an error whose text contains "your home does not exist on
  this machine", and that the sandbox's agent secrets are unchanged.
- **RDE-003** A chart tip placed by `placeTip` never rises above its plot: when its top would be
  above the plot's, it moves down until they meet (the horizontal clamp is unchanged).
- **RDE-004** The daily chart's e2e test runs on a pinned day (2026-11-02, the failing case), with
  the page's clock shifted but ticking, and checks the first and the last day's dates.

## Plan

- [x] Spec the missing-home test
- [x] Missing-home refusal asserted by an integration test
- [x] Mutations of the missing-root branch fail the test
- [x] Independent review approved and findings answered
- [x] Gates green with evidence in the spec
- [x] Daily chart tip stays inside the plot on a pinned tall weekday
- [x] Full e2e suite, gate and re-review of the chart fix

## Acceptance criteria

- The new test passes on dev's code.
- It fails when the `ENOENT` root branch's message is changed, and when that branch stops
  refusing (shown in Evidence).
- `scripts/dev ci` is green.
- The pinned daily-chart test fails on dev's `placeTip` (`pastPlot: 46`) and passes with RDE-003;
  `pipeline.e2e.ts` and the full e2e suite pass.

## Test plan

- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets_create::/)'`.
- Three temporary mutations of `open_folder`, each committed nowhere: the message reworded, the
  `ENOENT` root arm removed (the generic `ENOENT` arm answers), and a missing root created before
  the walk instead of refused.
- e2e `pipeline.e2e.ts`: the daily-chart test pinned to 2026-11-02 (RDE-004), red on dev's
  `placeTip`, then the file and the full suite with RDE-003.

## Decisions

- Remove the home outright (`remove_dir_all`) rather than moving it aside: the issue's case is a
  home that is gone, and nothing in the test needs it afterwards.
- No ADR: neither the test nor the tip's clamp takes a lasting decision.
- The chart fix rides in this PR rather than its own: it failed every PR's e2e job, this one's
  included, and the run must not open issues (#643's comment records it). It is one function and
  one test.
- Clamp, not flip below the point: the same rule as the horizontal clamp ("moved in so all of it
  stays inside"); the tip ignores the pointer (`pointer-events: none`), so covering the dot hides
  nothing the tip does not say.
- Pin the clock in the test rather than the mock's data: the test's subject is any day's tip, and
  a shifted `Date` keeps the app's and the mock's range in step (`period()` and `stageDays` both
  read it); it still ticks, so timers and "so far" stay sane.
- The Dashboard's tasks-per-day tip (`pages/dashboard.js`) places itself and has no top clamp
  either; it does not fail and is outside this change (said on #643).

## Evidence

- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets_create::/)'` on
  dev's code (a0d156d0, no production change): 6 passed, including
  `a_missing_home_is_refused_as_not_existing`.
- Mutations of `crates/yulius-app/src/vault/env_files.rs` (reverted with `git checkout`), each
  run with `test(=secrets_create::a_missing_home_is_refused_as_not_existing)`, exit 100, failing at
  the message assertion:
  - M1, the root `ENOENT` message reworded to "{within} is missing on this machine".
  - M2, the `Errno::ENOENT if visible.is_empty()` arm removed: the generic arm answers "the folder
    ~/ does not exist in your home".
  - M3, `std::fs::create_dir_all(&root)` before the walk (no refusal of the missing root): the
    call answers `{"text":"~/key does not exist","isError":true}`.
  Only `env_files.rs` changed in each, so the test reaches `open_folder`'s root branch, not the
  identical message of `secret_source_root`'s `canonicalize` refusal (the sandbox folder still
  exists; the test asserts it).
- `scripts/dev ci` at 504916a1: exit 0; nextest 2952 passed, 16 skipped; doctests, `cargo deny`
  (advisories, bans, licenses, sources ok), docs and macOS clippy passed.
- After the review's nit (the sandbox-folder assertion): the file's 6 tests pass, `scripts/dev
  lint` exit 0.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" — disabled, not
  passed.
- Independent review (fresh session, `om-code-review`) of 504916a1: APPROVE, no blocker or major.
  Answered: minor (Evidence missing, Plan unchecked, status) — this section, Plan and status
  updated; nit (Test plan said "Two … plus one") — reworded to three; nit (state in the test that
  the sandbox folder remains) — assertion added.
- Chart tip (RDE-003/004): PR #1012's CI run f6d16986 at f8f4daf0 failed only e2e, at the daily
  chart test (`pastPlot: 46`, "Mon, Nov 2"), as #1011's run 7d776c09 did; reproduced locally with
  `npx e2e run tests/pipeline.e2e.ts --grep "daily chart"`. A probe read the tip's box (left 1054,
  right 1227, top 665) against the plot's (right 1227, top 711): inside sideways, 46 px above.
- The pinned test (`clockAt` 2026-11-02 12:00 UTC) on dev's `placeTip`: exit 1, the same
  `pastPlot: 46` at 1280 px, after its first-day check ("Sun, Oct 4") passed, so the shifted clock
  reaches the app and the mock. With the top clamp: `tests/pipeline.e2e.ts` 14/14 passed.
- Full e2e suite (`npx e2e run`) at e7f8318d: 70 files passed, 1 skipped; 800 tests passed, 1
  skipped (36 min 42 s). After the typing nit (e544ecc9) the daily-chart test passes again.
- `scripts/dev ci` at e544ecc9: exit 0; nextest 2952 passed, 16 skipped; `cargo deny` ok.
- Independent review of e7f8318d (same fresh reviewer, new round): APPROVE, no blocker or major.
  Answered: minor (Evidence, Plan, status) — this; nit (`clockAt`'s argument type named only one
  `Date` overload) — typed as `[] | [string | number | Date] | ConstructorParameters<…>`
  (e544ecc9); pre-existing (the Dashboard's tasks-per-day tip has no top clamp) — out of scope,
  said on #643; note (two concerns in one PR) — recorded under Decisions; no other PR touches
  `stages.js` or `pipeline.e2e.ts`.
