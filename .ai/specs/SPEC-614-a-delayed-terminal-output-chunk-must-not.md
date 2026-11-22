---
yulius-task: c3476fc8-3918-478a-8717-50a49519ab50
status: in-progress
---

# SPEC-614 — A delayed terminal output chunk must not reach a replacement run

## Goal

Show on a task's live terminal only the output of the run whose screen it shows. Output that an earlier run produced, but that reaches the hub only after a replacement run attached, is dropped.

Part of #1624.

## Context

Each run's PTY reader in `crates/yulius-app/src/supervisor.rs` passes its chunks to `App::terminal_output` with the task ID. The redaction sink in `crates/yulius-app/src/vault/terminal.rs` looks up the task's current hub by that ID, and the 40 ms release of a held tail uses the same sink. `Hub::output` drops bytes while a parked screen is shown and accepts them again once a host attaches. A chunk of the previous run delayed in transit (a slow redactor load, the tail release timer, a reader that lost its run to an escalation) therefore draws on the replacement run's grid and goes to its viewers. An independent review of SPEC-558 found the ordering by reading the code; there was no runtime reproduction.

The supervisor already records the run whose screen the hub shows: `TaskRuntime::screen_of`, set by `attach_run` after it restores the hub (SPEC-301 LRS-001) and kept after that run ends until the next attach. Task #540 changes how parked screens are saved; this change keeps to the live output path and leaves the parking code as it is.

## Requirements

- **TRO-001** PTY output, including a released tail, reaches the hub only while the hub shows the screen of the run that produced it. Otherwise it is dropped, for the screen model and every viewer.
- **TRO-002** A host snapshot or reattachment of a run resets the hub only while the hub shows that run's screen.
- **TRO-003** The check and the hub change are one step against an attach: an attach restores the hub and records its run under the lock the check takes, so no chunk passes the check for the old run and then draws after the new run's restore.
- **TRO-004** The current run's output and the parked screen shown before a replacement attaches stay as they are.

## Plan

- [x] Specify binding live terminal output to its run
- [ ] Reproduce an old run's held chunk drawing on its replacement
- [ ] Reproduce a delayed tail release drawing on the replacement
- [ ] Bind output, snapshots and tail releases to the run that produced them
- [ ] Record the decision and update the architecture and changes documents
- [ ] Verify the touched suites and obtain independent approval

## Acceptance criteria

- **AC-1 (TRO-001, TRO-003, TRO-004)** Hold a synthetic output chunk of the first run between its host and the hub, end that run and attach a replacement for the same task, then release the chunk. The replacement's screen and the frames its viewers receive hold only the replacement's output, and the replacement's later output still reaches both.
- **AC-2 (TRO-001, TRO-004)** Make the first run print synthetic text that ends in the beginning of an agent secret, so its redactor holds a tail. Hold the tail's release, park the run, resume it in a replacement, then release the tail. The replacement's screen and frames do not show it. The parked screen shows until the replacement attaches.
- **AC-3 (TRO-002)** Snapshot and reattachment resets of a run that no longer owns the screen leave the hub as it is (by construction, the same check as TRO-001).

## Test plan

Both reproductions run in `crates/yulius/tests/parking/replaced_output.rs` with real processes: the supervisor, the detached `yulius ptyhost` and the fake agent. A session host wrapping the real one injects synthetic events into a run's own reader and can hold them until the test releases them. A test-only race point holds a tail's release. Each test fails on `dev` before the fix. Run the parking, supervision, secrets-redaction and terminal suites, the vault unit tests, repository quick checks, independent review and Yulius CI; request staging once after the merge.

## Decisions

- Bind output to the producing attempt with the existing `screen_of` record rather than a new run token in the hub. `screen_of` is already the supervisor's record of whose screen the hub shows, used to save a run's final screen and to check the Codex input screen, so one record answers all three. The hub stays agnostic of runs. ADR to be recorded with the fix.
- Hold `screen_of` while writing the hub (lock order `screen_of` → hub screen). `Hub` methods never call out while holding their lock, so no path takes the two in the other order.
- The reattach path still replaces the task's active writer with the old run's if the old run's host answers a reattach after a replacement attached. That is input, not output, and is outside this change; it is reported separately if reproduced.
