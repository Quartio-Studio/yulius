---
yulius-task: acd7ef52-4090-4440-b058-d9b436dee9ff
status: done
---

# SPEC-030: Attached images reach Claude Code as pasted images, not as paths

Status: implemented; for goal tasks superseded by [SPEC-117](SPEC-117-creating-tasks-work-in-a-very-very-weird.md); images are pasted where they sit in the text since [SPEC-124](SPEC-124-images-keep-their-position-in-the-text-f.md), not before it
(no typed first message: the `/goal` launch prompt names the attached files; a `/goal` carries no
image block). Prompt tasks still paste images, and a person's keys wait meanwhile.
Date: 2026-10-12
Owner: Yulius task #28
Scope authorization: user request ("I asked previous task to attach images as files, natively, but
it doesnt seem to work. Fix."), with a screenshot of a task terminal showing the first message as
`@"/…/pasted-….png"` followed by `Read 1 file`.

## Goal

An image attached on the new-task composer reaches the task's Claude Code session the way a person
pasting it would: the prompt shows `[Image #1]` and the first user message carries the image,
not a path the session reads with a tool.

## Context

- SPEC-025 (task #23, ADR-0022) mentions every attached file as `@"/path"` in the session's first
  message, given as the CLI prompt argument (`yulius_domain::request`, `supervisor.rs`).
- Symptom (the screenshot, and this task's own session): the terminal shows the long path and
  `Read 1 file`; the transcript records a `file` attachment.
- Root cause, measured on Claude Code 2.1.292 against a local mock of the Messages API (driver in
  Evidence): an `@` mention is attached as a synthetic Read tool call. The request carries
  `<system-reminder>Called the Read tool with the following input: {"file_path": …}` and the
  image as that call's result, before the text that still holds the path. Only a paste
  (bracketed paste of the image's path into the TUI) is attached natively: the prompt reads
  `[Image #1] …` and the user message has an `image` block. The CLI prompt argument never turns a
  path into an image: a bare path stays text. SPEC-025's evidence checked that an `image` block was
  sent, not how; its tests check the launch argument only, so nothing caught the difference.
- Claude Code takes a pasted path as an image when it matches `/\.(png|jpe?g|gif|webp)$/i` (read
  from its bundle). Text in the same input can still carry `@"/path"` mentions of other files.
  When the input ends on a mention, though, the file autocomplete opens and takes the Enter. It
  replaced the mention with a bare path and nothing was sent. A newline after the text avoids
  this (measured 4 of 4). Mentions placed right after `[Image #1]` are not attached.
- Pastes written in one burst lose the image (measured): each paste has to be taken before the
  next one is written.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PIMG-001 | A task's attached file is an image when its path ends in `.png`, `.jpg`, `.jpeg`, `.gif` or `.webp` (any case). Without images the first message is unchanged (SPEC-025 ATT-002/ATT-003). | `request.rs` unit tests |
| PIMG-002 | A fresh session of a task with attached images starts with no CLI prompt. Once its TUI reads input, Yulius pastes each image's path as its own bracketed paste, waiting after each until the screen shows `[Image #N]` or the output settled (at most 5 s). It then pastes the text, ending in a newline so the cursor is never on a mention, and sends Enter. In prompt mode the text is the request plus a quoted mention of each other file. In goal mode it is the SPEC-025 attachments message, with images left out of the mentions. | `request.rs` unit tests; `task_done.rs` tests |
| PIMG-003 | In goal mode, `/goal` is typed after the Stop hook of that typed first message (SPEC-025 ATT-003), and that Stop is not gated. | `task_done.rs` goal test |
| PIMG-004 | No control character reaches a paste: attachment paths with one are not attachments, and the text keeps only newlines and tabs (as SPEC-025). | `request.rs` unit test |
| PIMG-005 | Until the session submits it, the typed first message is pending. After a server restart a pending first message flags the task NeedsAttention with a notice; held for capacity and resumed on another account, it is typed again with its images in a fresh session (SPEC-068 OPN-001..003). | `first_message.rs` |

## Acceptance criteria

- AC-PIMG-1: a prompt-mode task created with `Make it blue` and `@/srv/shot.png` launches Claude Code
  with no prompt argument, and its terminal receives a paste of `/srv/shot.png`, then a paste of
  `Make it blue`, then Enter.
- AC-PIMG-2: a goal-mode task with an attached image gets the image pasted in its first message,
  then its `/goal` once that turn ended.
- AC-PIMG-3: a task with only a text file attached still mentions it in the CLI prompt (unchanged).

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Images reach Claude Code as pasted [Image #N] in both modes
- [x] Independent review, gates green and delivery to dev

## Test plan

- Unit (`yulius-domain` `request.rs`): image detection by extension; the opening of a task with
  images (typed: images and text, other files mentioned), without images (the CLI prompt as
  before), in goal mode (`None` without files); no control characters.
- Integration (`crates/yulius/tests/task_done.rs`, fake agent): the fake agent learns bracketed
  pastes like Claude Code (an image path becomes `[Image #N]`, text is kept), and
  `--echo-turn-ms` makes the turn on a typed message last. A prompt-mode task with an image
  launches with no prompt and the agent answers `[Image #1] Make it blue`. A goal-mode task with
  an image gets `[Image #1]`, and its `/goal` only after that 4 s silent turn ended, then
  `GOAL SET` once. Both fail before the fix, because the image is a mention in the launch
  prompt. The goal test also fails when the typed opening does not set the opening turn.
- Manual, outside the gate: the mock-API driver against the installed Claude Code (Evidence).
- Not covered: a live model answering about the image (needs a paid credential).

## Decisions

- Images are pasted into the TUI, the only channel measured to attach an image natively. The
  alternatives were keeping `@` mentions (a Read tool call, the reported defect), a bare path in
  the CLI prompt (stays text), and Claude Code's `--file` (Files API ids only). Recorded as
  ADR-0026, which supersedes ADR-0022's choice for images.
- Other files stay `@` mentions. That is how Claude Code attaches a text file natively; a pasted
  non-image path is only text.
- Only a session with images gives up the CLI prompt. Typing races a person's keys and depends
  on TUI readiness, so tasks without images keep the measured CLI path.
- Each paste waits for its `[Image #N]` on the screen, or for output to settle (500 ms quiet
  after a change). It never waits on a fixed sleep and gives up after 5 s. Without that wait a
  burst of pastes loses the image (measured).
- The first message is typed once, never retried: a retried paste could send the request twice.
- Review round 1 (fresh session) requested changes:
  - The goal test did not prove PIMG-003, because a pretend command-line turn cleared the
    opening flag. It now uses a silent typed turn and fails with the flag not set (checked).
  - Typing did not stop when its attempt ended. It now checks before each paste and before
    Enter; there is no test, since the window is a few seconds of a replaced run.
  - A server restart during typing and a held first message resumed on another account lose
    the message or its image. Rare; recorded in ADR-0026, follow-up issue #266. Fixed by
    SPEC-068 (task #61, ADR-0052): the first message stays pending until its `UserPromptSubmit`;
    a restart flags a task whose first message is still pending, and a held one resumes as a
    fresh session that types it again with its images.
  - Mentions inside a long, collapsed paste: measured, still attached.
  - A request starting with `/` plus an image is no longer a slash command (ADR-0026).
- The text paste ends with a newline: the autocomplete otherwise takes the Enter when the text
  ends on a mention (measured). The alternative of mentions first is not attached after an image
  placeholder (measured).

## Evidence

- Mock-API driver (manual, scratch only, not committed): a Python HTTP server on 127.0.0.1 answering
  `/v1/messages` with a fixed SSE reply and logging each request body; Claude Code 2.1.292 run in a
  PTY with `pexpect`, a scratch `HOME` (onboarding done, folder trusted, fake key approved),
  `ANTHROPIC_BASE_URL` set to the mock and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`.
  - `claude 'look at this\n@"/abs/red.png"'`: the user message is a `Called the Read tool`
    reminder, an `image` block and the text with the path (the defect).
  - `claude '/abs/red.png'` and `claude 'look at this\n/abs/red.png'`: text only, no image.
  - Paste `/abs/red.png`, paste `/abs/sp ace/my shot.png`, paste
    `look at this\nsecond line\n\n@"/abs/notes.txt"`, Enter (0.3 s apart): text
    `[Image #1] [Image #2]look at this…`, two `image` blocks, and the notes file's content attached.
  - The same pastes and Enter in one write: the image is lost.
  - The goal-mode text ending on `@"/abs/notes.txt"`, Enter after a 0.6 s pause (and the waits
    on `[Image #N]`): the autocomplete took the Enter and nothing was sent. With a newline after
    the mention (2 to 2.5 s pause): sent, the image block and the file's content attached, 4 of 4.
  - An image, then 40 lines of text ending on a mention and a newline: the full text, an `image`
    block and the file's content attached.
  - An image alone, then Enter: `[Image #1]` and an `image` block. An image then `Make it blue\n`:
    `[Image #1]Make it blue` and an `image` block.
- `cargo test -p yulius-domain --lib request`: 12 passed (5 new). `cargo test -p yulius-app --lib
  a_typed_first_message`: passed. `cargo test -p fake-agent`: passed.
- `cargo test -p yulius --test task_done`: 9 passed. Before the fix (commit 35746c7) the two image
  tests failed: the goal task launched with the mention, and the prompt task never showed
  `[Image #1]`. With `typed_opening.is_some()` replaced by `false` in `opens_with_turn`, the goal
  test fails ("the goal was typed during the turn on the first message").
- `scripts/dev ci` on 5eb9a4f: exit 0. Repository checks, fmt, clippy `-D warnings`, nextest
  (1173 run, 1173 passed, 16 skipped), doctests, cargo-deny (advisories, bans, licenses, sources
  ok), machete, docs. An earlier run's doctest failure (a missing `rustls` rlib) came from a
  concurrent build in the same target directory and did not recur.
- Review: round 1 requested changes (see Decisions), round 2 approved 5eb9a4f.
- Not tested here: a live model describing an attached image (no paid credential); the
  `[Image #N]` behaviour was checked against the installed Claude Code 2.1.292 with the mock API.
