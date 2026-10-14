---
yulius-task: 0226e2f8-0442-4493-9482-9199d48af1e4
status: in-progress
---

# SPEC-124: Images keep their position in the text, from paste to the agent

Status: in-progress
Date: 2026-10-14
Owner: task #105
Scope authorization: the person's request (task #105): images pasted or dropped in the task
composer become thumbnails in a row above the text, and the agent gets them apart from where the
person meant them. "People often write 'this part [image] looks wrong, while here [image] ...' and
the image's place in the text is the context." Make images position-aware end to end: inserted at
the caret or drop point, movable (drag and a keyboard way), deleted like a character with undo,
kept in place in the message the agent gets, in the stored prompt and in the original prompt
view; text-only and older prompts keep working. Builds on #102 (SPEC-117): one first message,
never a second one.

## Goal

A file pasted or dropped into the new-task composer sits in the text where it was put, as an
inline thumbnail (an image) or chip (another file). It can be moved and deleted like a character,
and it stays at that place everywhere the prompt goes: the stored prompt, the request file and
first message the agent gets, and the original prompt view.

## Context

- **Composer today** (`taskview.js` `mountComposer`, SPEC-014, SPEC-025, SPEC-026): a `<textarea>`
  with a tray of thumbnails above it (`composer-attachments`). `filedrop.js` `attachPromptDrop`
  uploads a dropped or pasted file to the sandbox's draft attachments and `drafts.js`
  `attachToDraft` adds it to `draft.attached`. At start, `withAttachments` appends one `@path` line
  per file after a blank line. Markdown keys come from `mdinput.js` (pure edit functions plus a
  textarea binding).
- **Stored prompt** (`tasks.prompt`): the text the composer sent. `yulius-domain`
  `Request::parse` treats every line that is only an `@path` mention (`@/a.png` or the POSIX-quoted
  `@'/a b.png'`) as an attachment and the rest as text, so position is lost at parse time.
- **What the agent gets** (SPEC-117, ADR-0095): in goal mode (the default) the `/goal` condition
  names the request file (`~/.yulius/tasks/<n>.md`, `request_markdown`) and the attached files; the
  request file has the text, then an "Attached files" list. In prompt mode without images the CLI
  prompt is the text then an `@"path"` mention per file (`first_message`); with images the first
  message is typed (`Opening::Typed`): each image path pasted on its own (Claude Code shows
  `[Image #N]`), then the text with other files mentioned, then Enter (`opening_pastes`,
  `type_opening`).
- **Original prompt view** (`actions.js` `openPromptDialog`, SPEC-043): `GET /api/tasks/{tid}/prompt`
  → `{text, attachments}`; the text renders read-only through the Markdown editor, the files as a
  list below it.
- Claude Code reads an `@` mention only after a space or at the start (an `@` glued to a word is an
  e-mail-like word), and an image path pasted into its prompt becomes `[Image #N]` at the cursor,
  wherever the cursor is (the fake agent models both).
- The task view's other text box, the queued-message box (SPEC-052), takes text only: no file can
  be added there today, so it is out of scope. A terminal already types a dropped file's path at
  the cursor.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| INL-001 | Pasting an image (or copied file) into the composer, or dropping files on it, inserts each uploaded file at the caret (the drop point for a drop), in the text, as an inline atom: an image as a thumbnail, another file as a chip with its name; no tray above the text. While it uploads the caret place is kept, so text typed meanwhile stays where it was typed. | e2e `inline-images.e2e.ts` |
| INL-002 | An inline file is one character for editing: arrows step over it, Backspace/Delete remove it whole, a selection takes it along. Undo and redo (⌘/Ctrl+Z, ⇧⌘Z / Ctrl+Y, the browser's Undo) restore every edit, inserts and moves of files included, one step per insert or move. Pasted text is always plain text. | e2e |
| INL-003 | A file moves by dragging it to another place in the text (the drop point is where it lands), and by the keyboard: select it (⇧ + arrow), cut (⌘/Ctrl+X) and paste (⌘/Ctrl+V) where it should go. A file cut or copied in the composer pastes back as the same file (no new upload); text copied out of the composer carries the file as its `@'path'` mention. A dragged selection moves too, its files with it. A file opens by double-click, or from the Preview, where each file is a button (the keyboard way). | e2e |
| INL-004 | The composer's prompt keeps the textarea's behaviour: Markdown keys of `mdinput.js` (list continuation, Tab indent, ⌘B/⌘I), ⌘/Ctrl+Enter starts, Preview, the placeholder, read-only while a task starts, disabled without write access, the accessible name "Task prompt" as a multi-line textbox, growing up to its cap then scrolling. It stays usable at phone width (no horizontal scroll; a thumbnail never wider than the box). | e2e `inline-images`, `responsive`, existing composer suites |
| INL-005 | The prompt the composer sends has each file at its place as an inline mention `@'<absolute path>'` (POSIX single quotes, always quoted). A draft (SPEC-026) stores the same text, so a reload restores each file in place; a draft from before this change (files only in `attached`) shows its files after the text. A file whose mention a person deletes leaves the draft. | e2e `drafts` |
| INL-006 | `Request::parse` keeps the order of text and files: an inline `@'<absolute path>'` anywhere in a line is a file at that place, and a line that is only an `@path` / `@'path'` mention is a file at that line (older prompts: their files follow the text). `text` stays the text without files (two spaces left by a removed inline file become one), used for titles, Jev's focus and the API's `text`. Control characters never reach a path or the agent, as before. | `request.rs` unit tests |
| INL-007 | The agent gets each file at its place. Request file (goal mode): the request text with each file written in place as `[file N: <path>]` (`N` its number in the "Attached files" list, which stays). Prompt mode without images: the text with each file mentioned in place as `@"<path>"`, a space added where the mention would touch a word. Prompt mode with images: the typed first message is pasted in order, text, then each image path where it sits (`[Image #N]` appears there), then the rest, then Enter: still one message (SPEC-117). An older prompt (files after the text) gets the same messages as before with three exceptions: its typed images now come after the text, where they are; a prompt written by hand with no blank line before its `@path` lines no longer gets one added; a path no mention can carry (`"` or `#`) is named in place, `[open with the Read tool: <path>]`, instead of in a list after the text. A mention right before a pasted image keeps a space, so the path never joins it. The goal condition is unchanged (ONE-002). | `request.rs`, `supervisor.rs` unit tests; `task_done.rs` `an_image_in_the_text_is_pasted_where_it_sits` |
| INL-008 | `GET /api/tasks/{tid}/prompt` adds `parts`: the prompt in order as `{"text"}` and `{"attachment": n}` items (`n` indexes `attachments`). The original prompt view shows each file in place inside the formatted text (an image as a thumbnail that opens full size, another file as a download chip); files after all the text show in the list below it, as before, so older prompts look unchanged. A task's title never carries a mention. | `task_prompt` integration test; e2e `original-prompt` |
| INL-009 | Text-only prompts are unchanged everywhere: same stored text, request file, first message and view. | `request.rs` unit tests; existing suites |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The stored request keeps each file at its place in the text, and the agent's request file and first message carry it there
- [ ] A typed first message pastes each image where it sits in the text
- [ ] The original prompt view shows each file in place, older prompts as before
- [ ] The composer holds files inline: paste or drop at the caret, delete and undo like a character
- [ ] Files move inside the composer by drag and by cut and paste, and the composer stays usable when squeezed
- [ ] ADR, architecture, API docs, earlier specs and the changes log say how positions travel
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green and the change delivered to dev

## Design

**One format, no migration.** The prompt stays one string. A file inside the text is the mention
`@'<path>'` (always single-quoted, so its end is unambiguous even next to punctuation); a whole
line `@path` stays a file at that line. Older prompts are files after the text, so they parse into
the same order model with no rewrite.

**Domain** (`yulius-domain::request`): `Request { text, attachments, parts }`, `parts` an ordered
`Vec<Part>` of `Part::Text(String)` and `Part::File(usize)`. Renderers walk `parts`:
`first_message` (prompt mode), the request file, and `Opening::Typed { pastes }` with
`Paste::Text` / `Paste::Image`, which `opening_pastes` turns into bracketed pastes (an image paste
waits for its `[Image #N]`; the last text paste ends with a newline, as before).

**Composer** (`promptinput.js`, new): a `contenteditable` element (`role="textbox"`,
`aria-multiline`) holding text nodes (`white-space: pre-wrap`) and atoms
(`<span contenteditable="false" data-path>`). Its model is a string with U+FFFC per atom plus the
ordered atoms, so the pure `mdinput.js` edits apply unchanged. The editor owns every structural
edit (Enter, paste, drop, cut, moves, Markdown keys) and its own undo history: typing stays native
and is recorded after each input (coalesced while typing a word), `historyUndo`/`historyRedo` and
the keys go to the history. Formatting commands are refused (plain text only).

## Acceptance criteria

- The e2e test types text, pastes an image in the middle, types more text, moves the image (cut
  and paste, and drag), starts the task, and the prompt the server received has the image's
  mention at the moved place; the original prompt view shows the image inside the text at that
  place.
- Backspace deletes an inline image in one press and ⌘/Ctrl+Z brings it back.
- A typed first message with an image in the middle shows `Look at [Image #1] here` in the fake
  agent, one message.
- Older prompts (`text\n\n@/a.png`) give the same request file, CLI prompt and view as before.
- At 360 px wide the composer has no horizontal scroll and an inline thumbnail fits.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Unit (`request.rs`): parse of inline, whole-line, legacy and malformed mentions; `text` without
  files; `first_message` mentions in place with spacing; request file markers; `opening` pastes in
  order; control characters.
- Unit (`supervisor.rs`): `opening_pastes` interleaves text and images.
- Integration (`task_done.rs`): a prompt-mode task with an image mid-text types it in place; a
  `task_prompt` read returns `parts`.
- e2e (`inline-images.e2e.ts`, mock mode): paste at caret, Backspace and undo, cut/paste move, drag
  move, start → received prompt; draft reload keeps positions; old draft appends; original prompt
  view in place; narrow width. Existing composer suites updated from `toHaveValue` to text checks.

## Decisions

- **Inline mention in the one prompt string** over a structured `attachments` field in the create
  API: no storage or API break, older prompts parse unchanged, and the agent-facing form is the
  `@` mention Claude Code already understands (ADR-0101).
- **`contenteditable` with own undo** over a textarea with `[Image #1]` text tokens: the request
  asks for the image inside the text; tokens would keep the thumbnails in a separate row. Native
  undo cannot record programmatic moves as one step, so the editor keeps its own history.
- **Files after all the text are listed below the text in the view**, so older prompts look as
  before and a file put at the end reads naturally.
- **Typed images are pasted where they sit**, older prompts included (images after the text, no
  longer first): one rule, and Claude Code attaches a pasted image wherever the cursor is.
- **An upload's placeholder is never an undo step**: when the files arrive (or the upload fails),
  every step of the history that holds the placeholder is rewritten, so undo goes from the
  inserted files straight to the text before them (review round 1).
- **A composer that is gone never writes the draft**: an upload that ends after the person left
  joins the draft through `attachToDraft` only, and the open composer, if any, shows it after its
  text (review round 1).
- **One pull request, about 1,100 changed lines outside tests**: over the ~400-line guide, kept
  whole on purpose. The parser, the agent-facing renderers, the editor and the view only make
  sense together. Shipping the parser alone would store inline mentions that no composer writes,
  and shipping the editor alone would send positions the agent drops. Each commit is a
  self-contained slice, and the review covered the whole range.
- **Accepted, not fixed**: a hand-crafted line of many unterminated `@'` mentions parses in time
  quadratic in its length. It is bounded by the 64 KiB prompt limit and comes only from the
  prompt's own author.
