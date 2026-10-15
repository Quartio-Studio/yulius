# 0101. Attached files keep their place in the prompt

## Status

Accepted. Specified in
[SPEC-124](../../.ai/specs/SPEC-124-images-keep-their-position-in-the-text-f.md). Refines
[0022](0022-native-task-attachments.md) (files mentioned after the text),
[0026](0026-attached-images-are-pasted.md) (images pasted before the text) and
[0021](0021-composer-drafts-in-the-browser.md) (a draft's files kept apart from its text); keeps
[0095](0095-a-goal-task-starts-with-its-goal-as-the-launch-prompt.md) (one first message).

## Context

A file pasted or dropped on the new-task composer became a thumbnail in a row above a textarea,
and the composer appended one `@path` line per file after the text. `Request::parse` took those
lines out, so the agent got the files apart from the words that referred to them. People write
"this part [image] looks wrong, while here [image] …": the image's place is the context, and other
agent UIs keep images inline for that reason.

## Decision

- **One string, the file in place.** The prompt stays one string. A file inside the text is the
  quoted mention `@'<absolute path>'` (POSIX single quotes, always, so its end is clear next to
  punctuation); a line that is only `@path` or `@'path'` is a file at that line, which is how
  every older prompt reads. `Request` keeps `parts`, the text and each file in order; `text`
  stays the text without files (titles, Jev's focus, the API's `text`). Storage, the create API
  and the queued-message flow are unchanged; nothing is migrated.
- **The agent gets each file at its place.** The request file of a goal task writes each file
  inside the text as `[file N: <path>]`, `N` its number in the "Attached files" list (files after
  all the text are only listed, as before). A prompt-mode CLI prompt mentions each file in place
  as `@"<path>"`, with a space before it (Claude Code reads a mention only after a space). A typed
  first message with images is pasted in order: text, each image's path where it sits, the rest,
  then Enter. Claude Code puts `[Image #N]` at the cursor, so the images land in place, still in
  one message. An older prompt's images, after its text, are now pasted after it, not first.
- **The composer holds files in its text.** The prompt is a `contenteditable` text box
  (`promptinput.js`) with text nodes and inline atoms. Its model is a string with U+FFFC per atom
  plus the atoms in order, so mdinput.js's pure Markdown edits apply unchanged. Typing stays
  native; structural edits (Enter, paste, drop, cut, moves, Markdown keys) are made on the model
  and drawn again. The box keeps its own undo history, because the browser's cannot record a
  programmatic move as one step. A file moves by drag, or by cut and paste. Copy writes a file as
  its mention, and pasting a mention of one of the prompt's files brings that file back.
- **The original prompt shows files in place.** `GET /api/tasks/{tid}/prompt` adds `parts`; the
  view draws each file inside the formatted text, and files after all the text in the list
  below, so older prompts look as before.

## Alternatives

- **A structured `attachments` field with positions in the create API.** It is exact, but it
  changes the API, the store and every reader of `prompt` (titles, intake, queued messages,
  automations), and older prompts would need a second path. The inline mention is the form Claude
  Code already reads.
- **A textarea with `[Image #1]` text tokens** (Claude Code's own TUI form). It keeps the native
  textarea, but the thumbnails stay in a separate row and the tokens break when edited
  character by character. The request asks for the image inside the text.
- **A rich-text editor library (ProseMirror and the like).** It has robust atoms and undo, but
  the UI ships plain ES modules with no build step (SPEC-002 ADR-01), and the composer needs only
  text plus atoms.
- **Native `contenteditable` undo.** It loses programmatic edits or splits a move into two steps;
  rejected for the editor's own history.

## Consequences

- A person who types `@'/abs/path'` by hand attaches that path, as a whole-line `@/path` always
  did; only the task's own attachment folders are ever served (SPEC-080).
- The web UI depends on Chromium-class `contenteditable` behaviour for atoms (caret around
  `contenteditable="false"`). The e2e suite covers paste, Backspace, undo/redo, cut and paste,
  drag, drop and a 360 px width.
- Tests of the composer read its text (`promptText` in e2e/tests/support.ts): it has no `value`
  attribute.
