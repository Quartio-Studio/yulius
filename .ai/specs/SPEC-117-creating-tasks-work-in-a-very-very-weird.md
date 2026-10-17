---
yulius-task: e824d53d-7796-4a84-8ff4-dc00dab16c08
status: done
---

# SPEC-117: A task starts with one message, before anyone can type

Status: done
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
| ONE-001 | A fresh goal-mode session starts with `/goal <condition>` as Claude Code's prompt argument: one message, nothing typed. No first message with the attached files comes before it, and no `/goal` is typed after it. The request file is written before the launch. Claude Code's record of the goal confirms it: none within `launch_goal_ms` (75 s, the typed goal's submit waits, once the session reads input; time held for capacity or an update not counted) flags the task, "Claude Code did not take the task's goal" (TSK-010). A server restart before the goal is recorded watches it again (SPEC-146 GWR-002). | `task_done.rs` `a_goal_task_starts_from_its_request_file`; `first_message.rs` `a_launch_goal_never_taken_flags_the_task`; `supervision.rs` AC-TSK-9 |
| ONE-002 | (Superseded by SPEC-212 WHL-001 to WHL-003: the launch condition is the whole request with each file in place; a typed one is one short line.) The goal condition of a task with attached files names them after the request file: `Everything requested in <file> is done. Attached files (open each with the Read tool; it shows images): <path>, <path>. <definition of done>`. Paths are one line each; no `@` mention. When the list would pass the cap the condition names the request file only (it lists them). The cap is 2000 characters (Claude Code 2.1.292 accepts 4000). A request without attached files keeps its condition. | `request.rs` unit tests |
| ONE-003 | A goal-mode run whose launch prompt is itself a `/goal` command never gets a `/goal` typed after it: also a resumed session whose held first prompt was that `/goal` (SPEC-068 hold, continued elsewhere). | `supervisor.rs` unit test `only_a_goal_launch_prompt_sets_the_goal`; `task_done.rs` (fresh launch) |
| ONE-004 | When the task's request file cannot be written, the session starts with no prompt and the task is flagged with the existing notice ("The task has no goal yet: type `/goal` …"), as before. A goal run an older server started, whose typed first message a hold kept across the restart into this version, is flagged too ("Yulius restarted before it set this task's goal"): no goal follows that message any more. | by inspection (request file); `first_message.rs` `a_goal_run_of_an_older_server_held_across_the_update_is_flagged` |
| ONE-005 | While Claude Code's folder-trust dialog shows on a fresh goal session, Yulius types nothing and says so in the terminal. When it is still unanswered after `trust_answer_ms`, the task is flagged; the notice says the task starts by itself once the dialog is answered (SPEC-106 OTR-002, kept). | `first_message.rs` `a_trust_prompt_answered_before_the_goal_keeps_the_goal`, `an_unanswered_trust_prompt_gets_nothing_typed_and_flags_the_task` |
| ONE-006 | A prompt-mode task with attached images still types its first message (SPEC-030: only a paste attaches an image natively). From the spawn until that message's Enter, a person's keystrokes (`terminal_input`, typing only) are not written to the session, except while the trust dialog shows. The first refused keystroke shows the notice "Yulius is typing this task's first message. Type once it is sent." Focus and mouse reports and wheel turns pass as before. | `first_message.rs` `keys_typed_while_yulius_types_the_first_message_wait`, `a_trust_prompt_before_a_typed_first_message_takes_the_answer` |
| ONE-007 | Resumed goal sessions (Continue, a switch, a resume past a usage limit) keep getting their goal typed after their opening turn, as today (SPEC-034, SPEC-068 TYP-003): Claude Code's goal is per session and those runs open with a continuation prompt. | `task_done.rs` `a_goal_the_tui_dropped_is_submitted_again`, `a_session_start_before_the_run_is_watched_still_types_the_goal_at_once`, the limit test; `first_message.rs` draft, focus and trust-word tests on a switched session |

## Acceptance criteria

(AC-ONE-1 and AC-ONE-2 give the goal forms of their time. SPEC-212 replaced them: the launch
`/goal` is the whole request with each file in place, then `Done = …`; a typed goal is one short
line.)

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
- [x] The goal condition names the task's attached files
- [x] A goal task starts with its /goal as the launch prompt, nothing typed
- [x] A person's keys wait while Yulius types an image task's first message
- [x] ADR, architecture and the earlier specs say how a task starts
- [x] Review rounds 1 and 2 in fresh sessions, findings fixed test-first
- [x] Review round 3 approves the final head
- [x] Every gate green, docs and the changes log updated

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
- **Review round 1** (fresh session) requested changes. A launch goal Claude Code never records
  went unflagged; it is now watched like a typed goal (`launch_goal_ms`). Three tests of the
  typed-goal retry and the early SessionStart passed without typing anything; they now run on a
  continued session. Minor fixes: older specs amended, the trust dialog in front of a typed first
  message tested, a goal run an older server left held across the update flagged at reconcile,
  attached paths kept as they are.
- **Review round 2** requested changes. The new launch-goal test took 80 s; the bound is now
  `AppConfig::launch_goal_ms` (75 s by default, 3 s in the test). Time a hook is held for an
  update no longer counts toward it, as capacity pauses already did. The reconcile flag waits while
  capacity holds the old first message: when capacity returns, a fresh session starts with its
  launch goal. The update-hold part has no test: the fake agent records its goal before its submit
  hook, so a held launch goal cannot be staged without reordering every goal test. It mirrors the
  tested capacity-pause path. A race on `typing_opening` was judged unreachable: a new run starts
  only after the old one is gone.
- **Review round 3** approved 5c374034. Its minor finding, a held launch goal not watched again
  after a server restart (not watched before this change either), is issue #453. A held launch
  goal resumed on another subscription without a recorded session starts with that `/goal` as its
  prompt and does not rewrite the request file: the earlier start wrote it in the same sandbox
  home, and the condition carries the definition of done.
- **Resumed sessions keep the typed goal.** They open with a continuation prompt (gate reasons,
  "Continue …"), which a `/goal` prompt would replace. Changing that is not part of creating a task.

## Evidence

- **Measurements.** Claude Code 2.1.292 was run against a local mock of the Messages API (scratch
  only, not committed): a Python HTTP server on 127.0.0.1 logging each `/v1/messages` body, Claude
  Code in a PTY driven by `pexpect`, a scratch `HOME` and `CLAUDE_CONFIG_DIR` with onboarding done
  and the folder trusted, `ANTHROPIC_BASE_URL` pointed at the mock.
  - `claude "/goal Make it blue | Done = merged"`: the first request holds `<command-name>/goal`,
    `Goal set: …` and the directive "A session-scoped Stop hook is now active with condition: …".
    The Stop hook evaluates the condition.
  - The same in an untrusted folder, after Down and Enter on the trust dialog: the goal is set.
  - A `UserPromptSubmit` command hook received `{"prompt": "/goal Make it blue | Done = merged"}`.
  - Typed `/goal …` with two pasted images: `<command-args>… [Image #1] [Image #2]`. The user
    message has only text blocks, `[Image: source: <path>]` per image, and no `image` block. The
    transcript agrees.
  - An image pasted before `/goal`: `[Image #1]/goal …` plus an `image` block, an ordinary prompt
    with no goal set.
  - `claude '/goal …\n\n@"/abs/notes.txt"'`: the mention stays text and the file is not attached.
  - From the 2.1.292 bundle: `/goal` is `local-jsx` and calls `lnt`; the condition cap `NEt=4000`
    gives "Goal condition is limited to 4000 characters".
- **Tests first.** `request.rs` (condition with files, cap fallback): 3 failed before the change.
  `fake-agent` `a_goal_prompt_argument_sets_the_goal`: failed before (`ECHO: /goal …` without
  `GOAL SET`). `task_done::a_goal_task_starts_from_its_request_file`: failed before (`prompt: None`).
  `first_message::keys_typed_while_yulius_types_the_first_message_wait`: failed before (the keys
  were mixed into the message). Review fixes: `a_launch_goal_never_taken_flags_the_task` and
  `a_goal_run_of_an_older_server_held_across_the_update_is_flagged` failed before their fixes.
  The latter also failed with the reconcile branch disabled (checked).
  `a_trust_prompt_before_a_typed_first_message_takes_the_answer` failed with the trust exception
  removed from `keys_wait_for_opening` (checked, then restored).
- **Gate.** `scripts/dev ci` on 5c374034: exit 0. Repository checks, script unit tests, `cargo
  check`, fmt, clippy `-D warnings`, nextest (1626 run, 1626 passed, 16 skipped), doctests,
  cargo-deny, cargo-shear, docs. An earlier run (0b648824) passed every test (1609), but its
  doctests failed: a `rustls` rlib was missing because my `cargo test` ran in the same target
  directory at the same time. It did not recur when nothing else built.
- **Review.** Fresh read-only sessions with the om-code-review skill. Round 1 (44c794fe) and round
  2 (0b648824) requested changes, all fixed (Decisions). Round 3 approved 5c374034. Its minor
  finding is issue #453; its nits are fixed after it (docs only).
- **Not tested here.** A live model reading an attached image with its Read tool, and a live
  Claude Code taking a launch `/goal` with a paid credential: both need a subscription. The
  mock-API runs above are the evidence for the CLI behaviour. The update-hold exclusion in the goal
  watch has no test (Decisions, round 2).
