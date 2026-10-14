---
yulius-task: e824d53d-7796-4a84-8ff4-dc00dab16c08
status: in-progress
---

# SPEC-117: A task starts with one message, before anyone can type

Status: in progress
Date: 2026-10-14
Owner: task #102
Scope authorization: the person's request (task #102): "Creating tasks work in a very very weird
way. First of all, images are pasted in the first messages and goal in the second, while images can
arrive in a goal. Moreover, firstly empty session is created and then messages are sent, this makes
it problematic because the user can type something in meantime. Everything should arrive before
user can type things and in a single message".

## Goal

A new task's session gets everything it needs (its goal, its request and its attached files,
images included) in one first message. That message is in before a person can type into the
session, so nothing a person types is mixed into it.

## Context

- **Today, goal mode** (the default; `supervisor.rs` `start_run`, `spawn_goal`; SPEC-011, SPEC-025,
  SPEC-030, SPEC-068). Claude Code starts with no prompt, or with a first message that mentions the
  attached files. Yulius waits until the TUI is ready, pastes each image (SPEC-030), waits until
  that turn's Stop hook, and only then types `/goal <condition>`. The person sees an empty session,
  then a first message with the images ("Its goal follows in the next message"), then a second
  message with the goal. Anyone can type into the terminal during the whole sequence; SPEC-068
  only made Yulius leave such a draft alone and flag the task (#252).
- **Today, prompt mode**: without images, the request is the CLI prompt argument (one message,
  nothing typed). With images, Yulius types the first message into an empty session (SPEC-030).
- **Measured on Claude Code 2.1.292** with the installed binary against a local mock of the
  Messages API (driver in Evidence):
  - `claude "/goal <condition>"` sets the goal from the prompt argument. The first request holds
    the `/goal` command, `Goal set: …` and the goal's directive ("A session-scoped Stop hook is now
    active … immediately start"). The turn runs, and the Stop hook judges the condition. Nothing is
    typed. The same happens after the folder-trust dialog is answered. `UserPromptSubmit` fires with
    the prompt `/goal <condition>`, so Yulius's submit hook (capacity holds, SPEC-068) sees it.
  - A `/goal` typed with pasted images shows `[Image #1]` in the prompt box and in the condition,
    but the request carries no `image` block. The model gets only a text `[Image: source: <path>]`
    per image. An `@"/path"` mention inside a `/goal` is not attached either.
  - Images pasted before `/goal` make the input an ordinary prompt (`[Image #1]/goal …`): no goal is
    set.
  - Claude Code 2.1.292 caps a goal condition at 4000 characters ("Goal condition is limited to
    4000 characters", from its bundle); Yulius caps its conditions at 450 (an older 500 cap).
- So no single goal message can carry an image block natively. What a `/goal` can carry is each
  image's path, which the model opens with its Read tool (it shows images). The request file
  already lists the attached files under "Attached files" (SPEC-011).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ONE-001 | A fresh goal-mode session starts with `/goal <condition>` as Claude Code's prompt argument: one message, nothing typed. No first message with the attached files comes before it, and no `/goal` is typed after it. The request file is written before the launch. | `task_done.rs`, `request.rs` |
| ONE-002 | The goal condition of a task with attached files names them after the request file: `Everything requested in <file> is done. Attached files (open each with the Read tool; it shows images): <path>, <path>. <definition of done>`. Paths are one line each; no `@` mention. When the list would pass the cap the condition names the request file only (it lists them). The cap is 2000 characters (Claude Code 2.1.292 accepts 4000). A request without attached files keeps its condition. | `request.rs` unit tests |
| ONE-003 | A goal-mode run whose launch prompt is itself a `/goal` command never gets a `/goal` typed after it: also a resumed session whose held first prompt was that `/goal` (SPEC-068 hold, continued elsewhere). | `task_done.rs` |
| ONE-004 | When the task's request file cannot be written, the session starts with no prompt and the task is flagged with the existing notice ("The task has no goal yet: type `/goal` …"), as before. | unit-free: same path as before, `supervisor.rs` |
| ONE-005 | While Claude Code's folder-trust dialog shows on a fresh goal session, Yulius types nothing and says so in the terminal. When it is still unanswered after `trust_answer_ms`, the task is flagged; the notice says the task starts by itself once the dialog is answered (SPEC-106 OTR-002, kept). | `first_message.rs` trust tests |
| ONE-006 | A prompt-mode task with attached images still types its first message (SPEC-030: only a paste attaches an image natively). From the spawn until that message's Enter, a person's keystrokes (`terminal_input`, typing only) are not written to the session, except while the trust dialog shows. The first refused keystroke shows the notice "Yulius is typing this task's first message. Type once it is sent." Focus and mouse reports and wheel turns pass as before. | `first_message.rs` |
| ONE-007 | Resumed goal sessions (Continue, a switch, a resume past a usage limit) keep getting their goal typed after their opening turn, as today (SPEC-034, SPEC-068 TYP-003): Claude Code's goal is per session and those runs open with a continuation prompt. | existing `task_done.rs`, `first_message.rs` tests, moved onto a resumed session |

## Acceptance criteria

- AC-ONE-1: a goal task created with `Make the header blue` and `@/srv/shots/header.png` launches
  Claude Code with the prompt `/goal Everything requested in …/tasks/<n>.md is done. Attached files
  (…): /srv/shots/header.png. Done = …`. Its terminal shows `GOAL SET` once, and nothing was typed
  before it.
- AC-ONE-2: a goal task without attached files launches with `/goal <request> | Done = … | Full
  request: …` as its prompt; no `/goal` is typed into it.
- AC-ONE-3: a prompt task with an image: keys typed by a person while Yulius pastes the image never
  reach the session's first message; the message is `[Image #1] Make it blue`, and the person is
  told to type once it is sent.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The goal condition names the task's attached files
- [ ] A goal task starts with its /goal as the launch prompt, nothing typed
- [ ] A person's keys wait while Yulius types an image task's first message
- [ ] ADR, architecture and the earlier specs say how a task starts
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Test plan

- Unit (`yulius-domain` `request.rs`): the condition names attached files (images and others, in
  order, one line, no `@`); a list that would pass the cap falls back to the request file alone; a
  request without files keeps the inline or named form; never longer than the cap. The prompt-mode
  opening is unchanged.
- Integration (`crates/yulius/tests/task_done.rs`, fake agent): the fake agent runs a `/goal` prompt
  argument the way Claude Code does (`GOAL SET`, the goal turn), and the fake harness passes a
  `/goal` launch prompt through, like Claude Code. A goal task with an image launches with the
  `/goal` prompt and its file list; `GOAL SET` shows once; no `ECHO:` of a typed first message comes
  before it. The tests fail before the change (the launch had no prompt and the image was typed).
- Integration (`first_message.rs`): a prompt task with an image whose person types during the paste.
  The keys do not reach the message, and the notice shows. Fails before the change (the keys join
  the message). The trust tests run on a goal task whose `/goal` waits behind the dialog. The draft,
  focus and trust-words tests move to a resumed goal session, the path that still types its goal.
- Not covered: a live model reading an attached image (needs a paid credential); the mock-API
  measurements are the evidence that Claude Code takes the prompt argument.

## Decisions

- **`/goal` as the launch prompt** for a fresh goal session. Alternatives: (a) keep typing,
  everything in one typed `/goal` message with pasted images: Claude Code drops the image blocks of
  a `/goal` (measured), so it buys nothing over paths, and the typing window stays; (b) set the
  goal through generated settings (a prompt-type Stop hook) and keep the typed image message: no
  `goal_status` records, which Yulius's goal verdicts read (ADR-0063), and still two steps. The
  launch prompt is the only path with nothing typed. Recorded as ADR-0095, which supersedes the
  goal-mode parts of ADR-0022 and ADR-0026.
- **Images in a goal are paths the agent opens.** A `/goal` cannot carry an image block. The
  condition lists the files and says the Read tool shows images. That is what Claude Code itself
  gives the model for an image pasted into `/goal` (`[Image: source: <path>]`), minus the typing.
- **The cap rises from 450 to 2000 characters.** Claude Code 2.1.292 accepts 4000; half leaves room
  for a CLI that lowers it again, and paths of a few images fit. The inline request stays at 150
  characters, so short requests keep their condition.
- **Prompt mode keeps typing images and blocks keystrokes meanwhile.** Prompt mode still has no
  other way to attach an image natively (SPEC-030). Refused keystrokes with a notice beat queued
  ones: replayed keys could include Esc or Ctrl+C, which would interrupt the turn they land in.
- **Resumed sessions keep the typed goal.** They open with a continuation prompt (gate reasons,
  "Continue …"), which a `/goal` prompt would replace. Changing that is not part of creating a task.

## Evidence

(Filled in when the task is done.)
