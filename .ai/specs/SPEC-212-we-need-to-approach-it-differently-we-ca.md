---
yulius-task: f35669fb-b2c2-4718-9d5e-397c387b6548
status: in-progress
---

# SPEC-212: A goal task's first message is the person's whole request

Status: in-progress
Date: 2026-10-17
Owner: task #199
Scope authorization: the person's request (task #199), with a screenshot of a session whose first
message reads `/goal Everything requested in /var/lib/yulius/sandboxes/…/home/.yulius/tasks/1.md is
done. Done = repository changes merged into dev …`: "we need to approach it differently, we can't
send something like this, rather the whole prompt. We can use prompt templates ofc but research
whether we can inject the instruction in more native way and implement".

## Goal

A goal task's session starts with the person's own request, whole, as its `/goal`. Yulius's
instructions (the definition of done, how to read attached files) reach the agent and the goal's
judge through Claude Code's native context (the SessionStart briefing). The first message no longer
points at a file in the sandbox home.

## Context

- **Today** (SPEC-011, SPEC-117 ONE-002, ADR-0095; `request.rs` `goal_condition`). A fresh goal
  session is launched with `/goal <condition>` as Claude Code's prompt argument. A request of up to
  150 characters without files becomes `<text> | Done = … | Full request: <file>`. Anything longer,
  or with an attached file, becomes `Everything requested in <file> is done. [Attached files …]
  Done = …`. The cap is 2000 characters. The request file (`~/.yulius/tasks/<n>.md`) holds the
  request, the attached files, the full definition of done and the waiting rule. The briefing
  (SessionStart `additionalContext`) quotes 600 bytes of the request and carries the delivery rules.
  A resumed goal session (Continue, a switch, a limit) gets the same condition typed into its TUI.
- **Measured on Claude Code 2.1.292** (installed binary, scratch home, a local mock of the Messages
  API logging each request body; driver in Evidence):
  - There is no native way to set a goal other than the `/goal` command: no CLI flag and no setting.
    `lnt()` in the bundle registers a session `Stop` hook of type `prompt` with the condition and
    records `goal_status`. A prompt-type Stop hook in settings writes no `goal_status`, which Yulius's
    goal verdicts read (ADR-0063, ADR-0095).
  - The `/goal` argument is `args.trim()` of the command line (`Yve()` splits only the name off). A
    multi-line launch prompt keeps its line breaks, backticks and quotes in `<command-args>`, in
    `Goal set: …` and in the condition. A 3803-character multi-line condition was set. The cap is
    `NEt=4000`, compared with the JavaScript string length (UTF-16 code units).
  - The goal's judge (the Stop hook) gets the conversation transcript, **including the SessionStart
    hook's `additionalContext`** as a system reminder, then "Condition: <condition>". So the
    briefing reaches the agent and the judge alike. The judge decides on the condition: a definition
    of done that is only in the briefing is context, not the condition it rules on.
  - **Typed** into the TUI as one write, `/goal` plus 693 characters is the command. 828 or more
    characters arrive as a plain user prompt (Claude Code takes a long write as a paste), so no goal
    is set. A bracketed paste starting with `/goal` is a plain prompt too. A typed condition must
    stay short; today's typed form can reach 2000 characters with attached files.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| WHL-001 | A fresh goal session's launch condition is the person's request as written, all its lines kept, each attached file at its place as `[file N: <path>]` (a file after all the text on its own line), followed by a blank line and the short definition of done (`Done = …`). Control characters other than line breaks and tabs become spaces. No request-file path appears unless the request is cut (WHL-002). A request with no text is its files. | `request.rs` unit tests |
| WHL-002 | The launch condition is at most 4000 UTF-16 code units, Claude Code's own measure. A longer request is cut at a character boundary, and the condition says where the rest is: `…` and `(The request continues in <request file>.)`, then the definition of done. The request file is still written for every goal task, unchanged. | `request.rs` unit tests |
| WHL-003 | The condition typed into a resumed goal session is one line of at most 700 UTF-16 code units: the request on one line, then ` | Done = …`, when that fits. Otherwise it is `Everything requested in <request file> is done. Done = …`, cut to the bound. It names no attached files: the request file lists them. | `request.rs` unit tests; `task_done.rs` typed-goal tests |
| WHL-004 | The briefing's task section carries the definition of done natively: `Definition of done: <short definition of done> End with a short summary: what changed, how it was verified, and what could not be tested in this environment.`. A request with attached files adds that each is opened with the Read tool (it shows images). Both are mandatory parts, so they are never cut. | `briefing.rs` unit tests |
| WHL-005 | Nothing else about starting a goal task changes: one launch prompt, nothing typed, the launch-goal watch, the trust dialog handling, prompt mode, queued messages and the request file (SPEC-117 ONE-001, ONE-003 to ONE-007). | existing `task_done.rs`, `first_message.rs`, `supervision.rs` tests |

## Acceptance criteria

- AC-WHL-1: a goal task created with `Fix the header\n\nIt overlaps @'/srv/a.png' on mobile.`
  launches Claude Code with the prompt `/goal Fix the header\n\nIt overlaps [file 1: /srv/a.png] on
  mobile.\n\nDone = repository changes merged into dev on green Yulius CI, …`, and its briefing
  carries `Definition of done:` and the Read-tool line.
- AC-WHL-2: a 6000-character request launches with a condition of at most 4000 UTF-16 units that
  ends with the request-file pointer and `Done = …`.
- AC-WHL-3: a resumed goal session of the same task gets a one-line condition of at most 700 units
  typed, which Claude Code takes as the `/goal` command.

## Plan

- [x] Read the product context, docs and earlier sessions on the topic
- [x] Measure Claude Code's /goal natively against a mock API
- [ ] Spec and ADR for a goal that carries the whole request
- [ ] The launch /goal carries the whole request with its files in place
- [ ] A typed goal stays short enough to be a command
- [ ] The briefing carries the definition of done natively
- [ ] Docs, changes and every gate green
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Merge into dev on green CI

## Test plan

- Unit (`yulius-domain` `request.rs`): the launch condition keeps the request's lines and puts each
  file at its place; the definition of done follows a blank line; no request-file path for a request
  that fits; a long request is cut within 4000 UTF-16 units (emoji counted twice) with the pointer
  and the definition of done; control characters go; a request of only files. The typed condition is
  one line, at most 700 units, falls back to the named form, and never lists files.
- Unit (`briefing.rs`): the task section has the definition of done (with deploys, and the playground
  form). The Read-tool line shows only with attached files. Both survive a tight budget.
- Integration (`crates/yulius/tests/task_done.rs`, fake agent): the launch prompt of a goal task is
  `/goal <request>\n\nDone = …` with no request-file path. The typed goal of a resumed session is one
  line. Existing goal-start tests updated to the new form.
- Not covered: a live model judging a long condition (needs a paid credential). The mock-API
  measurements are the evidence for Claude Code's behaviour.

## Decisions

- **The whole request is the `/goal` condition; the instructions go to the briefing.** The
  alternatives were measured or read from the 2.1.292 bundle:
  (a) The request as a plain launch prompt, with the goal set elsewhere. Claude Code has no other
  goal entry point, and a settings Stop hook loses `goal_status` (ADR-0095).
  (b) The request plus the full definition of done in the condition. The full definition is about
  1000 characters of rules that the briefing already carries, and every judge call repeats it.
  (c) `--append-system-prompt` for the instructions. It is native too, but Yulius's per-task context
  already lives in the SessionStart briefing (SPEC-002 §7.5), which Claude Code re-runs after a
  compaction, and the judge sees it (measured). A second channel would split the briefing.
  (d) A `UserPromptSubmit` `additionalContext` on the first prompt. It is attached to that message
  only and does not survive a compaction as the briefing does.
  The short `Done = …` stays in the condition: it is what the judge rules on, and the person
  allowed a template. Recorded as ADR-0168 (amends ADR-0095's condition).
- **The cap is Claude Code's 4000, counted as it counts.** The request is the condition now, so the
  2000 margin of SPEC-117 would cut ordinary requests. Counting UTF-16 code units, as JavaScript's
  `length` does, makes 4000 exact. A Claude Code that lowers its cap refuses the goal, and the task
  is flagged as before.
- **The typed goal stays short and one line.** It is typed as keystrokes. Over about 800
  characters Claude Code takes it as a paste, and pasted input is not parsed as a command
  (measured). A line break would submit it early. A resumed session already holds the whole request
  in its conversation, so naming the request file there costs nothing. This also fixes the typed
  form of today, which could reach 2000 characters with attached files.
- **The request file stays.** It is the full record for a cut request and the fallback of the typed
  goal, and SPEC-011 tools may read it. Writing it costs nothing.
