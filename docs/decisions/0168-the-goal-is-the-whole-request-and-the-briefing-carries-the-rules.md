# 0168. The goal is the whole request, and the briefing carries the rules

## Status

Accepted. Specified in
[SPEC-212](../../.ai/specs/SPEC-212-we-need-to-approach-it-differently-we-ca.md). Amends the
condition of [0095](0095-a-goal-task-starts-with-its-goal-as-the-launch-prompt.md): the launch
prompt is still `/goal <condition>`, with nothing typed.

## Context

A goal task's first message read `/goal Everything requested in /var/lib/yulius/sandboxes/…/tasks/1.md
is done. Done = …`. The person who wrote the task saw a file path where their request should be, and
asked for the whole prompt, with Yulius's instructions injected in a more native way.

Measured on Claude Code 2.1.292 against a local mock of the Messages API:

- `/goal` is the only way to set a goal. A prompt-type Stop hook in settings writes no
  `goal_status`, which Yulius's goal verdicts read (ADR-0063).
- A launch `/goal` keeps a multi-line condition as written. The cap is 4000 UTF-16 code units.
- The goal's judge reads the transcript, the SessionStart `additionalContext` included, and rules
  on the condition.
- A `/goal` typed as keystrokes is taken as a paste, and so not as a command, once it passes about
  800 characters.

## Decision

- The launch condition is the person's request as written: every line kept, each attached file at
  its place as `[file N: <path>]`. A blank line and the short `Done = …` follow. It is cut only past
  4000 UTF-16 code units, and then it points at the request file for the rest.
- The rules travel natively in the SessionStart briefing, which the agent and the judge both read:
  the definition of done, the closing summary and, when files are attached, that the Read tool opens
  them. The Git and delivery and waiting rules were already there.
- A goal typed into a resumed session is one line of at most 700 code units: the request when it
  fits, else `Everything requested in <request file> is done. Done = …`.
- The request file is still written for every goal task.

## Alternatives

- **The request as a plain prompt and the goal set another way.** Claude Code has no other goal
  entry point that records `goal_status`.
- **The full definition of done inside the condition.** It adds about 1000 characters of rules that
  the briefing already holds, and every judge call repeats them.
- **`--append-system-prompt`, or a `UserPromptSubmit` `additionalContext`.** Both are native. The
  first splits Yulius's per-task context over two channels. The second is attached to one message
  and does not survive a compaction, which re-runs SessionStart.

## Consequences

- The session's first message is the person's request; only the `Done = …` line is Yulius's.
- Conditions are longer, so each judge call reads more (at most 4000 code units, as before).
- A resumed session's typed goal may name the request file. Its conversation already holds the
  whole request.
- A Claude Code that lowers its goal cap below a request's length refuses the goal, and the task is
  flagged ("did not take the task's goal").
