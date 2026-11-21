---
yulius-task: f09ba136-9aa7-4298-8509-879a8ff4faab
status: done
---

# SPEC-594. Keep several unfinished new-task drafts

## Goal

A person can set aside the new-task prompt shown in task #578, including its images, write another, and return to either before starting a task. Fixes #1602.

## Context

The composer in [SPEC-026](SPEC-026-if-i-type-something-here-and-i-change-ta.md) reopens a single live object and storage key for a user and sandbox. Writing a second request through that object replaces the first. The browser tests exercise independent sandboxes and navigation with one draft in each. Task #578 asks for several in one sandbox. [Issue #1602](https://github.com/Quartio-Studio/yulius/issues/1602)

## Requirements

- **MDR-001** The new-task composer lets a writer create a fresh draft and choose an unfinished draft by its prompt or attachment name. Creating and switching drafts launches no task.
- **MDR-002** Each draft keeps its text, inline files, tray and existing per-draft task choices across switches, navigation and reload. The selected draft reopens in that tab. Closing an untouched editor adopts newer edits saved in another tab. A draft saved by an older browser version remains editable. Model and effort follow the existing browser preferences.
- **MDR-003** Successful task creation and confirmed discard remove just the chosen draft. A reopened draft stays read-only while its task starts. A failed start keeps it and releases it for editing. A pending upload belongs to its original draft, including after a switch, and cannot restore a discarded draft. Sign-out forgets all drafts.

## Plan

- [x] Reproduce the single-draft behavior in the new-task composer
- [x] Cover independent drafts and legacy recovery in the browser
- [x] Let people create, switch and remove individual task drafts
- [x] Preserve edits when reopening pending and shared drafts
- [x] Verify the composer and obtain independent approval for delivery
- [x] Verify the four CI failures with bounded test fixtures
- [x] Obtain independent approval of the CI follow-up

## Acceptance criteria

- **AC-1 (MDR-001/002)** Open a legacy draft with an image and Goal selected. Create two more, switch among them, reload, and recover the image, text and mode of the first.
- **AC-2 (MDR-003)** Cancel a discard, confirm it, then start another draft. The remaining draft can still be edited and started. A failed start preserves all drafts.
- **AC-3 (MDR-002/003)** Release a held upload after switching and after discarding its owner. The file appears only in its surviving owner. Sign-out removes stored drafts.

## Test plan

Exercise the real composer in Chromium against the mock API in `drafts.e2e.ts`. Use held uploads and failed creation for recovery. Run the existing composer and web suite, quick repository checks, security command, independent review and Yulius CI.

Run the Rust delivery approval cases, the two real-Git merge-train cases and the parked-owner stall case named by CI run `7a94e552-efe3-49a9-875c-07b467d03a2e`. Keep the CI test deadlines and each behavior assertion.

## Decisions

Extend the existing browser draft storage with separate keys for additional drafts. This preserves the original key and allows independent writes from different tabs. Keep the selected draft per tab. Attachments follow the existing server retention and browser quota behavior. [ADR-0433](../../docs/decisions/0433-store-each-new-task-draft-under-its-own-key.md)

## Evidence

- `spec_check` validated the task spec. The independent scope review returned proceed.
- On the baseline, the same user and sandbox returned the identical draft object. The three-draft and held-upload browser regressions failed looking for the New draft button.
- A simulated storage event reproduced a stale inactive draft in the picker. The browser regression failed to find the updated name before inactive drafts began following other tabs' storage events.
- The held-upload test caught the upload status covering New draft above the prompt. Draft controls sit below the composer so a pending upload leaves them reachable.
- Independent review reproduced an untouched open editor restoring a draft discarded in another tab. The two-tab regression failed with the original storage key present after a held upload and a switch. Remote removal now retires the open editor and its pending upload. An unmount flushes only dirty drafts.
- `scripts/dev quick` passed. Vidoc was disabled by the project. `scripts/dev mutants` reported no changed Rust code to mutate.
- Round two reproduced a held first upload surviving a discard, and a held upload lost when another tab erased the prompt. Discard and successful start now send an explicit cross-tab retirement message, including for an empty draft. Erasing text remains an edit to the same draft. Three two-tab scenarios cover these distinct outcomes.
- Round three reproduced an editable draft during held creation and stale text after leaving an untouched editor. The success, failed-start and cross-tab regressions all failed at the reviewed head. The pending state now belongs to the draft across remounts. An untouched editor adopts a deferred remote save on release, preserving live files and later local edits.
- Independent review approved `91c18a861ffdd7b76b9e8437b738e1a2b4736597` after running all 25 Chromium tests in `drafts.e2e.ts`. All passed, including legacy restore, three simultaneous drafts, inline image recovery, asynchronous submission and two-tab retirement and editing.
- `git diff --check`, `scripts/dev quick` and `spec_check` passed. The previous full local web runs were interrupted. The full suite runs again at the approved implementation, and task #578's delivery record tracks the exact-head Yulius CI verdict.
- Yulius CI run `7a94e552-efe3-49a9-875c-07b467d03a2e` at `dc3dd3f04` failed four Rust integration tests in shard 1. The approval test exceeded 120 seconds and two real-Git merge-train tests exhausted their 100-second fixture deadlines. The parking test expected a later stall notice while the PTY agent could still be answering the first diagnosis. All four passed in a serial run of the original test binary. [Issue #1606](https://github.com/Quartio-Studio/yulius/issues/1606) · [Issue #1610](https://github.com/Quartio-Studio/yulius/issues/1610)
- After merging current `dev`, `cargo nextest run --profile ci` passed all nine focused Rust cases without retries. The six approval cases took 4.4–9.5 seconds each. The isolated bisection and arm-order cases took 17.2 and 23.8 seconds. With the first diagnosis answered, the stall check delivered its single status notice in the parking case. `scripts/dev quick` passed.
- `scripts/dev mutants --base e98ddccc5` reported “No mutant in the Rust lines changed against e98ddccc5.” The Rust follow-up touched integration tests only. `scripts/dev security` reported the project's Vidoc scan disabled.
- Independent review approved `9696823d39dde240a5fea91862c20ae7c30bbd30` with no findings. The reviewer compared the composer and its tests with the earlier approval, inspected all four CI fixes, and reran the parking test successfully in 1.79 seconds.
