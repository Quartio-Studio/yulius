---
yulius-task: 5ff9fbda-c027-4c25-a9ee-a0886ea15533
status: in-progress
---

# SPEC-298: "Reload from disk" after a save conflict shows the disk's text or says why not

Status: in-progress
Date: 2026-10-24
Owner: Yulius task #284
Scope authorization: issue [#571](https://github.com/Quartio-Studio/yulius/issues/571) (found by
the independent review of SPEC-181, task #168): "Reload from disk" either shows exactly what is on
disk, or says why it could not, and keeps the modified mark. Requirement amended:
[SPEC-181](SPEC-181-here-ui-got-bugged-a-little-bit-we-want.md) EDT-015.

## Goal

After a save conflict (409), "Reload from disk" never leaves the person's own text in the editor
marked "Saved": it shows the disk's text, or it keeps the text marked modified and says why.

## Context

- Symptom: in `crates/yulius-server/web/assets/editorview.js`, the conflict handler of `save` ran
  `tab.dirty = false; await refreshFromDisk(tab, { reload: true })`. `refreshFromDisk` then
  returned without replacing the text in three cases:
  - the disk is back at the version the tab knows (`file.sha256 === tab.sha`): it returned before
    `replaceText`, although `reload` had already asked for the whole file;
  - the file is no longer text (`file.content == null`);
  - the file was deleted (404) or the server unreachable: the `catch` said nothing.
  In each the person's unsaved text stayed in the editor, the status bar said "Saved" and the tab
  lost its ●, so the next save could write it without the conflict question.
- Root cause: the modified mark was cleared before, not after, the text was replaced, and the
  early returns made for the periodic look (EDT-015: an edited text is kept) also applied to the
  person's explicit reload.
- Why the tests missed it: the only conflict test reloads a file that changed to a new version.
- `replaceText` already makes a reload one undoable edit of the part that changed.

## Requirements

- RLD-001: "Reload from disk" clears the modified mark only once the disk's text is in the editor
  (or in the kept text of a suspended tab), never before the read.
- RLD-002: it replaces the text with the disk's text whatever the disk's version, including the
  one the tab already knows, as one undoable edit (EDT-015).
- RLD-003: when the file was deleted (404), the text stays, stays marked modified and a toast says
  "The file was deleted"; when it is binary or too large now, the toast says so in the words used
  when such a file is opened; any other failure shows "Couldn’t reload <path>" with the error.
- RLD-004: a look that starts while a reload is on its way (the page or the Editor back in front,
  the tab activated) takes its place as a reload; a save makes it moot, and a moot reload says
  nothing (review of SPEC-298).
- RLD-005: the periodic look and the look on activation behave as before.

## Plan

- [x] Spec for the reload-after-conflict fix
- [x] Failing e2e tests: deleted file and same version after a 409
- [x] Reload from disk keeps the modified mark unless it loaded the disk's text
- [x] Independent review answered
- [ ] Gates green and docs updated

## Acceptance criteria

- AC-1 (e2e): with the 409 dialog open the file is deleted; "Reload from disk" keeps the text,
  the status bar says "Modified", the tab keeps its ●, and a toast says "The file was deleted".
- AC-2 (e2e): with the 409 dialog open the file goes back to the tab's version; "Reload from
  disk" shows the disk's text, marked "Saved"; one undo brings the person's text back.
- AC-3: the existing EDT-015 tests (reload to a new version, looks while saving) still pass.

## Test plan

- `e2e/tests/editor.e2e.ts`: two tests next to "unsaved edits are kept when the disk moves on",
  using the mock's file tree (`__yuliusMock.repoFile`, and new `removeRepoFile`, `repoFileTooLarge`
  and `holdRead` hooks). The first two fail before the fix: the status says "Saved" and the text is
  the person's. Review: a too-large file's toast, and a look started while the reload's read is held
  (`visibilitychange`), which asks for the whole file and shows the disk's text.
- The full e2e suite and the repository gate (`scripts/dev ci`), and `scripts/dev security`.

## Decisions

- The fix lives in `refreshFromDisk` (`reload` skips the "same version" and "edited" early returns
  and clears the mark itself after `replaceText`), not in a second reload function: one path reads
  the disk and replaces the text, so the undo, line-ending and presence handling stay shared.
- A deleted file keeps the text modified rather than closing the tab: the person's text is the
  only copy left, and saving again goes through the conflict dialog (Overwrite recreates it).
- Keys typed between the click on "Reload from disk" and its answer are replaced too (one undo
  brings them back): the person chose to lose their changes (review nit).
- A newer look carries the reload (`tab.reloading`) rather than being refused while one is on its
  way: the newest look still wins (EDT-015), and no look of the person's choice is lost.
- No ADR: this restores the intended behavior of an existing requirement.
