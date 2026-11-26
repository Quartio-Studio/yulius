---
yulius-task: d872f7f7-d8d8-4403-92b5-85685b9ab892
status: in-progress
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
  file's one- or two-line `//!` doc, the `impl` / `mod tests` wrappers around moved items, the visibility the move needs (`pub(super)`,
  re-exports `pub(crate) use` for paths other files already name), and rustfmt re-wrapping a
  line that the visibility made too long.
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
- [ ] Independent review of PR (b) answered
- [ ] Start admission and queue drain moved into supervisor/start.rs
- [ ] Launch moved into supervisor/launch.rs
- [ ] Run-dir markers and opening/goal typing moved into supervisor/typing.rs
- [ ] Independent review of PR (c) answered
- [ ] Subscription switching moved into supervisor/switch.rs
- [ ] Attach and watch loop moved into supervisor/watch.rs
- [ ] Stop and close moved into supervisor/stop.rs
- [ ] Placement/limits moved into capacity.rs and viewer input into terminal.rs
- [ ] Independent review of PR (d) answered

## Acceptance criteria

- `supervisor.rs` keeps the shared types and state (`TaskRuntime`, `SupervisorState`, run-start
  types, `runtime()`) and the module declarations. The moved code lives in the new files.
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
  `supervisor/park.rs`, and `crate::parking` keeps when to park.
- Parent code imports moved items by name (`use codex::{..}`), instead of using `super::*` globs
  in the children, so every file states what it uses.
