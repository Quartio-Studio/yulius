# 0095. A goal task starts with its `/goal` as the launch prompt

## Status

Accepted. Specified in
[SPEC-117](../../.ai/specs/SPEC-117-creating-tasks-work-in-a-very-very-weird.md); supersedes the
goal-mode parts of [0022](0022-native-task-attachments.md) (a first message mentioning the
attached files before the goal) and [0026](0026-attached-images-are-pasted.md) (images pasted into
that first message), and the goal typed after a restart-held first message of
[0052](0052-only-keystrokes-count-as-typing.md). Prompt mode is unchanged.

## Context

A new goal task (the default mode) opened as an empty Claude Code session. Yulius waited until the
TUI was ready and typed a first message holding the attached files, with images pasted. After that
turn ended it typed `/goal <condition>`. The person saw an empty session and then two messages,
and could type into the terminal throughout. A draft typed then kept the goal out and flagged the
task (#252). They asked for everything to arrive in a single message, before anyone can type.

Measured on Claude Code 2.1.292 against a local mock of the Messages API:

- `claude "/goal <condition>"` sets the goal from the prompt argument and runs the turn. Nothing is
  typed, `UserPromptSubmit` fires with the `/goal` prompt, and a folder-trust dialog answered first
  does not stop it.
- A `/goal` with pasted images shows `[Image #N]`, but the request carries no image block: the
  model gets `[Image: source: <path>]` text. An `@` mention in a `/goal` is not attached either.
- The goal condition is capped at 4000 characters.

## Decision

A fresh goal session is launched with `/goal <condition>` as its prompt argument, and nothing is
typed into it. The condition names the request file and, when they fit in 2000 characters, the
attached files as plain paths for the Read tool, which shows images. No other first message is
sent and no `/goal` follows. Claude Code's record of the goal confirms it, as it confirmed a typed
one: none within the goal's submit waits (75 s once the session reads input, time a hold keeps
the prompt not counted) flags the task. A launch whose prompt is a `/goal` (also a held one continued in
another session) never gets one typed. A folder-trust dialog in front of it is still announced,
and still flags the task when nobody answers it. A prompt-mode task with images still types its
first message, the only native image path. From the spawn until its Enter, a person's keystrokes
are dropped with one notice, except into the trust dialog. Resumed goal sessions keep the typed
`/goal` after their continuation turn.

## Alternatives

- **One typed message, `/goal` plus pasted images.** Claude Code drops the image blocks of a
  `/goal`, so this gives the model the same paths. It keeps the typing window the person
  complained about.
- **The goal as a prompt-type Stop hook in the generated settings, the typed image message
  kept.** Claude Code would write no `goal_status` records, which Yulius's goal verdicts read
  (ADR-0063). The task would still start in two steps.
- **Queue a person's keys during a typed first message and replay them after it.** Esc or Ctrl+C
  in the queue would interrupt the turn they land in. Dropping them with a notice is predictable.

## Consequences

- An attached image reaches a goal session as a path the agent opens with its Read tool, not as an
  image block in the first message. No `/goal` message can carry one in Claude Code 2.1.292. The
  request file lists the attached files too.
- A goal task starts at once: no readiness wait, no typed message and no draft race at its start.
  The `opening-pending` marker and the restart handling of SPEC-068 now apply to prompt-mode tasks
  with images only. A goal run that an older server started, with its first message held across
  the update that deploys this, gets no goal typed after the hold: it is flagged instead
  ("Yulius restarted before it set this task's goal").
- Conditions may be up to 2000 characters, so a Claude Code with a lower cap would refuse a long
  one. The task is then flagged ("did not take the task's goal").
- Prompt-mode keys typed before a first message with images is sent are lost and the person is
  told so.
