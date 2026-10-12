# 0019. Attached files reach Claude Code as `@path` mentions in its first message

## Status

Accepted. Specified in
[SPEC-025](../../.ai/specs/SPEC-025-uploaded-attachements-should-be-clickabl.md); replaces the
"open each one with the Read tool" list of SPEC-014.

## Context

Files attached on the new-task composer reached the agent as a list of paths to open with the
Read tool, so the model saw text, not the image. Measured on Claude Code 2.1.292 against a local
mock of the Messages API: an `@path` mention in the CLI prompt argument becomes an `image` block
(a text file becomes its content); an image pasted into `/goal …` is dropped; a `/goal` typed
while a turn runs is queued; an `@path` typed at the end of the TUI input opens the file
autocomplete, which takes the Enter.

## Decision

The attached files are mentioned (`@/path`, or `@"/path"` with whitespace) in the session's first
message, given as the CLI prompt argument. In prompt mode that message is the request followed by
the mentions. In goal mode a task with attached files starts with a short message holding the
mentions and saying the goal follows; Yulius then types `/goal` as before. The request file still
lists the paths.

## Alternatives

- Paste image paths into the TUI after `/goal` (Claude Code turns them into `[Image #N]`): images
  only, races a person typing, mid-turn delivery of a queued image not measured.
- Paste them into the `/goal` line: Claude Code drops them.
- Claude Code's `--file`: takes Files API ids, not local files.

## Consequences

- The model sees the files before it plans, without a tool call.
- A goal-mode task with attachments spends one short extra turn before its goal.
- Behaviour depends on Claude Code's `@` mention handling; the mock-API driver in SPEC-025 checks
  it again after a Claude Code upgrade.
