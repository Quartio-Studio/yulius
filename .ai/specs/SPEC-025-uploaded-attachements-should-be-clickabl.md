---
yulius-task: 21113514-aeb7-4592-a8bb-89140d60bc5b
status: in-progress
---

# SPEC-025: Attachments open when clicked and reach Claude Code natively

Status: in progress
Date: 2026-10-12
Owner: Yulius task #23
Scope authorization: user request ("uploaded attachements should be clickable. Moreover, the attachement should be natively passed to claude code, not as a path"), with a screenshot of a composer attachment chip.

## Goal

A file attached on the new-task composer can be opened by clicking its chip, and the task's
Claude Code session receives each attached file as a native attachment (an image as an image
content block), not as a path the agent has to open with the Read tool.

## Context

- SPEC-006 / SPEC-014: files dropped or pasted on the composer are uploaded to the sandbox's
  drafts folder and shown as chips (`taskview.js` `renderTray`); at start the composer appends one
  `@path` line per file (`filedrop.js` `withAttachments`). The chips cannot be clicked.
- SPEC-014 "finished by hand": an `@path` typed inside `/goal` never reached the model, so
  `yulius_domain::request` now turns the `@path` lines into a list "open each one with the Read
  tool" in the first message (prompt mode) and in the request file (goal mode, the default). The
  model only sees paths.
- Measured on Claude Code 2.1.292 against a local mock of the Messages API (no credential, no paid
  call; driver in the Evidence section):
  - an `@/abs/path.png` mention in the CLI prompt argument sends an `image` block in the first
    user message; a text file mentioned the same way is attached as its content (a Read result);
    `@"/path with spaces/x.png"` works too;
  - an image path pasted into `/goal …` is dropped: the condition gets the literal `[Image #1]`
    and the model only `[Image: source: …]` text;
  - a `/goal` typed while the first turn still runs is queued and set when it ends;
  - an `@path` typed or pasted at the end of the TUI input opens the file autocomplete, which
    takes the Enter (so typing mentions into the terminal is unreliable).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ATT-001 | Each composer attachment chip is a button ("Open <name>"). An image opens in a preview dialog at full size (closed by Esc, × or a click outside). A text file (HTML, XML and scripts included) opens in a new tab as plain text, a PDF, audio or video as it is, anything else downloads: an attached file never runs as the Yulius site. The remove button only removes. | e2e `filedrop.e2e.ts` |
| ATT-002 | A prompt-mode task's first message is its text followed by one mention per attached file on its own line, always quoted (`@"/path"`). A path no mention can carry (a `"` or `#`) is listed after "Open with the Read tool:" instead. An `@` line with a control character is not an attachment. | `request.rs` unit tests |
| ATT-003 | A goal-mode task with attached files starts its session with a first message (the CLI prompt) that says the files belong to the task and its goal follows, then mentions each file as in ATT-002. `/goal` is typed once that turn's Stop hook arrived (at most 5 minutes later), and that Stop is not gated. Without attached files the session starts with no first message, as today. | `request.rs` unit tests; `task_done.rs` goal tests |
| ATT-004 | The request file still lists the attached paths, saying they were given in the session's first message (open one again with the Read tool if needed). | `request.rs` unit test |

## Acceptance criteria

- AC-ATT-1: clicking an image chip shows the image in a dialog; Esc closes it and the chip is still attached.
- AC-ATT-2: a goal task created with `@/srv/shots/header.png` launches Claude Code with a prompt mentioning `@/srv/shots/header.png`, and its goal is still set.
- AC-ATT-3: a prompt-mode task's first message ends with the mention lines and has no Read-tool list.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A prompt-mode task's first message attaches its files natively as @mentions
- [x] A goal-mode task's files reach Claude Code natively in a first message before /goal
- [x] Composer attachment chips open the file when clicked
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Test plan

- Unit (`yulius-domain` `request.rs`): mention formatting (plain, whitespace-quoted), first message
  of prompt mode, the goal-mode first message (`None` without files), the request file wording.
- Integration (`crates/yulius/tests/task_done.rs`, fake agent): a goal task with an attached file
  is launched with a prompt carrying the mention (the fake harness records the launch mode in
  `fake-launch.txt`) and its goal is still set; a goal task without files has no prompt.
- Browser e2e (`e2e/tests/filedrop.e2e.ts`, mock server): clicking an image chip opens the preview
  dialog, Esc closes it, the attachment stays; the remove button does not open the preview.
- Manual, outside the gate: the mock-API driver against the installed Claude Code (Evidence).
- Not covered: a live model answering about the image (needs a paid credential).

## Decisions

- Native attachment through `@path` mentions in the CLI prompt argument, in both modes. It is the
  one channel measured to deliver an image block and text files, and it needs no typing into the
  terminal. Alternatives: pasting image paths into the TUI after `/goal` (works as a queued
  message, but only for images, races a person typing, and mid-turn delivery was not measured);
  pasting them into `/goal` (dropped by Claude Code); Claude Code's `--file` (downloads Files API
  ids, not local files).
- Goal mode spends one short extra turn (the attachments message) before `/goal`. Accepted: the
  model sees the files before it plans, and a `/goal` typed while that turn runs is queued
  (measured). Recorded as ADR-0019. Yulius types `/goal` after that turn's Stop hook, not after
  quiet output (a silent tool call is quiet too), and lets that Stop end the turn without gates,
  so the goal is typed once and between turns.
- Images preview in the existing modal so the composer keeps its state. A tab opened from a blob
  URL shares the page's origin and the CSP does not restrict scripts, so other files open only as
  plain text or as types that cannot run script (PDF, audio, video); the rest download (review
  finding).
- Mentions are always quoted: Claude Code 2.1.292 reads an unquoted mention only up to its last
  letter or digit and a `#` as a line range (read from its bundle; quoted mentions verified with the
  mock API).

## Evidence

(filled in at the end)
