# 0026. Attached images are pasted into the first message, not mentioned

## Status

Accepted. Specified in
[SPEC-030](../../.ai/specs/SPEC-030-i-asked-previous-task-to-attach-images-a.md); supersedes
[0022](0022-native-task-attachments.md) for images. Other files are still `@path` mentions.
Amended by [0052](0052-only-keystrokes-count-as-typing.md) (SPEC-068, #266): a restart during the
typing flags the task, and a held first message keeps its images.

## Context

ADR-0022 mentioned every attached file as `@"/path"` in the CLI prompt argument. The person saw
the path and `Read 1 file` in the task's terminal. Measured on Claude Code 2.1.292 against a
local mock of the Messages API, an `@` mention is attached as a synthetic Read tool call: a
`Called the Read tool` reminder with the image as its result, and the path still in the text.
Only a bracketed paste of an image's path into the TUI attaches it as a person pasting it would:
`[Image #N]` in the prompt and an `image` block in the user message. Three other facts were
measured:
- The CLI prompt never turns a path into an image.
- Pastes written in one burst lose the image.
- An `@` mention at the cursor opens the file autocomplete, which takes the Enter.

## Decision

A fresh session of a task with attached images (`.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, the
paths Claude Code pastes as images) starts with no CLI prompt. Once its TUI reads input (its
SessionStart hook, then quiet output), Yulius types the first message:
1. Each image's path goes as a bracketed paste of its own. Yulius waits until the screen shows
   `[Image #N]`, or until the output changed and settled (at most 5 s).
2. The text follows as one paste ending in a newline: the request, or in goal mode the "files
   attached" message, with the other files as `@` mentions.
3. Then Enter.

The message is typed once and never retried. A goal task gets its `/goal` after that turn's
Stop hook, as before. Tasks without images keep the CLI prompt.

## Alternatives

- Keep `@` mentions for images: the reported defect (a Read tool call, the path shown).
- A bare path in the CLI prompt: stays text.
- Claude Code's `--file`: takes Files API ids, not local files.
- Paste every attachment as a session without images would: a pasted non-image path is plain
  text, so other files stay mentions.
- Type every first message: it races a person typing and depends on TUI readiness, for no gain
  when there is no image.

## Consequences

- The model gets each image as a pasted image, before it plans; the terminal shows `[Image #N]`.
- A session with images depends on TUI readiness and paste handling. A person typing during
  start-up can mix their input into the first message. A first message the TUI lost is not
  retyped (the request file still lists the files).
- Typing stops when its attempt ends, so a replacement session never gets half a message. The
  first message stays pending (`opening-pending` in the run directory) until the session's
  `UserPromptSubmit`. A server restart during the few seconds of typing leaves it pending: the
  new server flags the task NeedsAttention with a notice, and a person finishes the message
  (ADR-0052).
- A first message held for capacity and resumed on another account starts a fresh session there
  that types the task's opening again, images included; the hook's text (`[Image #1] …`) would
  carry no image (ADR-0052).
- A request starting with `/` and an attached image is no longer run as a slash command: the text
  follows the pasted image.
- The mock-API driver in SPEC-030 checks the behaviour again after a Claude Code upgrade.
