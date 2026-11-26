---
yulius-task: d872f7f7-d8d8-4403-92b5-85685b9ab892
status: done
---

# SPEC-666: Cleanup: split supervisor.rs into modules (verbatim moves)

## Goal

Carry out finding F1 of the November 2026 audit of the task supervision core (L1): split
`crates/yulius-app/src/supervisor.rs` (10,132 lines, twelve responsibilities) into a
`supervisor/` module directory, so `supervisor.rs` becomes a small module root and the code
lives in files named by what they do. Behaviour unchanged.

## Context

- The owner's cleanup charter makes this a chore. It asks for no spec. Yulius needs this file to
  deliver code changes, so it is the task's plan record only.
- The layout follows `delivery.rs` + `delivery/`: `supervisor.rs` declares `mod codex;` and
  others, and the moved code reaches the parent's private items as a child module.
- Two follow-up tasks wait for these merges: the logic findings (F2, F6, F7, F9-F13) and the
  harness seam (F3). Neither is done here, and `TaskRuntime` is not shrunk here.

## Requirements

- SPL-001: each commit moves code verbatim. The only edits are imports, `mod` lines, a new
  file's one- or two-line `//!` doc, the `impl` / `mod tests` wrappers around moved items,
  the visibility the move needs (`pub(super)`, re-exports `pub(crate) use` for paths other
  files already name), and rustfmt re-wrapping a line that the visibility made too long.
- SPL-002: visibility is as narrow as the move allows.
- SPL-003: a moved item's unit tests move with it.
- SPL-004: each pull request is reviewable with `git diff --color-moved`. The four pull requests
  are (a) codex and antigravity, (b) end and park, (c) start, launch and typing, (d) switch,
  watch, stop and the capacity/terminal moves. Each starts from fresh `origin/dev` after the
  previous one merged.

## Plan

- [x] Codex code moved into supervisor/codex.rs
- [x] Antigravity code moved into supervisor/antigravity.rs
- [x] Independent review of PR (a) answered
- [x] Run end, loss and reconcile moved into supervisor/end.rs
- [x] Parking a session moved into supervisor/park.rs
- [x] Independent review of PR (b) answered
- [x] Start admission and queue drain moved into supervisor/start.rs
- [x] Launch moved into supervisor/launch.rs
- [x] Run-dir markers and opening/goal typing moved into supervisor/typing.rs
- [x] Independent review of PR (c) answered
- [x] Integration gate repaired: parking::a_staging_outcome_reaches_its_task_once green on dev
- [x] Subscription switching moved into supervisor/switch.rs
- [x] Attach and watch loop moved into supervisor/watch.rs
- [x] Stop and close moved into supervisor/stop.rs
- [x] Placement and limits moved into supervisor/placement.rs, viewer input into supervisor/input.rs
- [x] Independent review of PR (d) answered

## Acceptance criteria

- `supervisor.rs` keeps the shared types and state (`TaskRuntime`, `SupervisorState`,
  `runtime()`) and the module declarations; the run-start types went to `start.rs`. The moved code lives in the new files.
- In every pull request, a line-multiset comparison of removed and added lines leaves only the
  edits SPL-001 allows, and `git diff --color-moved` shows the same.
- `cargo check`, `cargo clippy -D warnings` and the whole `yulius-app` lib test binary pass,
  with the same number of tests as before. Yulius CI is green on each merged head.

## Test plan

No new tests: behaviour does not change. The existing unit tests move with their code and
must still pass. The `yulius` integration binary runs in Yulius CI.

## Decisions

- Tests move into a `#[cfg(test)] mod tests` of the new file, with their `super::` paths
  unchanged, so they stay verbatim.
- A cluster the audit sends to an existing sibling file (`parking.rs`, `capacity.rs`,
  `terminal.rs`) goes there only when the move needs no `pub(crate)` on the supervisor's
  private state. Otherwise it becomes a child module of `supervisor/`, so its visibility stays
  `pub(super)`. Parking a session uses `TaskRuntime`'s `run`, `intent`, `parked_activity` and
  `terminal_cleanup`, the launch lock and the `EndIntent`/`SwitchEnd`/`ActiveRun` types. So it is
  `supervisor/park.rs`, and `crate::parking` keeps when to park. For the same reason placement,
  limits and rotation (which need `TaskRuntime`'s `run`, `SupervisorState`'s `runtimes` and
  `ObservationSource`) are `supervisor/placement.rs` rather than `crate::capacity`, and viewer
  input (`run`, `prompt_receipts`, `input_seq`, `viewer_submits` and three private types) is
  `supervisor/input.rs` rather than `crate::terminal`.
- A moved item that other files name as `crate::supervisor::X` keeps that path through a
  re-export of the same visibility (`pub use` for `pub` items, `pub(crate) use` otherwise), so no
  caller outside `supervisor/` changes. Sibling files under `supervisor/` import from the
  module that defines the item (`super::typing::X`).
- An accepted Vidoc finding (`[[ignore]]` in `.config/vidoc.toml`) follows its code to the new
  file in the same pull request, with its `line` updated where it has one.
- Parent code imports moved items by name (`use codex::{..}`), instead of using `super::*` globs
  in the children, so every file states what it uses.

## Evidence

Four pull requests, each a set of verbatim-move commits reviewed with `git diff --color-moved` and
a removed-versus-added line multiset (only the edits SPL-001 allows remain):

- (a) #1772, merged at `0c8ad78e1`: `codex.rs`, `antigravity.rs`.
- (b) #1777, merged at `7a3b2f468`: `end.rs`, `park.rs`.
- (c) #1782, merged at `b1fc903d1`: `start.rs`, `launch.rs`, `typing.rs`, and the repair of the
  integration gate that Yulius assigned to this task (`parking::a_staging_outcome_reaches_its_task_once`,
  #1712: reproduced 1 of 40 under `--stress-count` on 4 contended CPUs, a probe showed the task
  Working at the failing read; with the test waiting for the release, 80 of 80).
- (d) this pull request: `switch.rs`, `watch.rs`, `stop.rs`, `placement.rs`, `input.rs`.

`supervisor.rs`: 9,284 product and 848 test lines on dev `90a749384`; 1,087 product and 249 test
lines after (d), the rest in twelve child modules. For every part:
`scripts/cargo clippy -p yulius-app --all-targets -- -D warnings` clean,
`RUSTDOCFLAGS="-D warnings" scripts/cargo doc -p yulius-app --no-deps` clean (from part c on),
`scripts/cargo nextest run -p yulius-app --lib` 740 passed (the same count before and after each
move), `scripts/dev quick` passed, and an independent review of the exact head. The `yulius`
integration binary, e2e and the test job ran in Yulius CI on each merged revision
(merge-first). Vidoc is switched off for this project: the moved `[[ignore]]` entries could not
be exercised by a scan.

