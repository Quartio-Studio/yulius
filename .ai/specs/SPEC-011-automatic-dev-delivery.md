---
yulius-task: 7f2dac7e-b5cd-454e-b06b-370a2a983dae
status: done
---

# SPEC-011: tasks start from their request and finish on dev by themselves

Status: done
Date: 2026-10-12
Owner: Yulius tasks #10, #12 and #16, finished by hand after their transcripts were analysed
Scope authorization: user requests (F 2026-10-12): "Dev deployment/merge should be automatic,
test-driven, and not wait for an explicit human approval" (task #10); "the process should always
be for features fixes etc: spec -> worktree + branch from dev -> implementation + tests -> review
-> fix -> iterating on tests -> merge to dev and deploy to dev (if deployment exists) -> tests and
stuff ... this should finish the goal, without any input and summarise. Then the user in different
task can ... merge to prod and deploy to prod"; an uploaded image "passed to claude as path"
(task #12); a goal "written ... but didnt start" (task #16); "it should be opus-5.5"; "auto
compact can be enabled".

## Goal

A task runs from its request to a merge into `dev` (and a deploy to dev where one exists), tested,
with a summary, and no person in between. It starts reliably with everything it was given (images
included), it knows when it is done, it waits for CI without polling, and it never promotes to
production unless asked.

## Context

The transcripts of all 17 tasks of the `yulius` sandbox (2026-10-11/12) showed why tasks did not
finish on their own:

- **No reachable end.** The `/goal` was the raw prompt. Claude Code's evaluator then demanded a
  production deployment that the profile left to a person (`dev` had `verify = manual`), or called
  a local fix "met". Agents sat idle for hours or looped: task #8 had 497 Stop-hook blocks and 386
  refused `delivery_merge` calls while CI was queued, until its context filled and the session
  died.
- **Goals not set.** `/goal` was typed before Claude Code read input (tasks #13 to #16 waited 2–4
  minutes for a person to press Enter). A prompt over 450 characters never got a goal at all
  (tasks #6, #7, #9 and #17).
- **Images as text.** An image attached to a short prompt was only an `@path` inside `/goal`.
- **Polling.** Agents waited for CI with `sleep` loops and `pgrep -f` monitors that matched
  themselves and never ended (complaint 1).
- **The briefing told them to promote,** and it was over Claude Code's hook-context limit, so
  agents saw only a 2 KB preview of it.
- **The model.** Every task ran Sonnet, because Yulius passed no model. Auto-compact was off in the
  sandbox settings.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DLV-009 | The `standard` preset has no environment: a merge into `dev` on green checks (DLV-005) is the whole integration stage. Its test gate (`require_checks`) stays on. | `delivery::standard_*`, `tests/delivery.rs` |
| DLV-010 | A stored or `yulius.toml` `standard` profile that still holds only the former manual `dev` checkpoint (no command) reads as the new `standard` profile; any other environment is untouched; `product` keeps its manual checkpoints. | `delivery::legacy_standard_*`, `spec_format::config` |
| DLV-011 | `delivery_merge` called while the checks of the reviewed head still run arms the merge: Yulius merges exactly that head when they pass (after each CI run and from the delivery loop), tells the agent, and drops the armed merge (telling why) when the branch moved on or the merge is refused. Failing checks are still refused. | `tests/delivery.rs` `an_armed_merge_happens_when_the_checks_pass` |
| DLV-012 | `yulius wait` (the agents' shell command; `POST /internal/agent/wait` with the task token) returns when the Yulius CI checks at the agent's head end, its delivery stage changes, an armed merge is decided or a notice arrives that the session has not seen; it reads only Yulius's records. | `wait_cli` tests, the armed-merge test |
| TSK-010 | A goal task's request file (`<sandbox home>/.yulius/tasks/<number>.md`: the request, its attached files to open, the definition of done, how to wait) is written at each start, and `/goal` names it; a short request without files is also quoted in the condition (≤ 450 characters). | `request` unit tests, `task_done.rs` `a_goal_task_starts_from_its_request_file` |
| TSK-010 | `/goal` is typed after the SessionStart hook and quiet output, confirmed by Claude Code's goal record, retried (Enter, retype, Enter; 5–30 s apart); a goal never taken flags the task (needs attention, notice in the terminal). | `task_done.rs` |
| HRN-012 | A task that names no model runs Claude Code's `opus` (the latest Opus); auto-compact is on for every run (run settings outrank the sandbox's `settings.json`); Bash defaults to 10 minutes, at most 30. | `yulius-harness` launch tests |
| COM-004 | The briefing stays under 9,500 bytes (Claude Code keeps larger hook context in a file and shows a 2 KB preview), lists tools by name, ends the delivery path at the merge into the integration branch and its environments, says never to promote unless asked, and tells agents to wait with `yulius wait`. | `briefing` tests |

## Design

- **Definition of done.** The definition (`yulius_domain::request::DoneDefinition`) comes from the
  delivery profile:
  - repository changes merged into the integration branch on green Yulius CI, deployed and
    verified on the integration environments that have a command;
  - work needing no repository change reports its result (research, a brainstorm, CD set up
    outside the repository: the flow is not forced on it);
  - a closing summary;
  - never a promotion unless the request asks.
- **Attachments.** The composer adds `@path` lines. `Request::parse` lifts them out into
  "Attached files: open each one with the Read tool", which shows images to the model.
  (Superseded by SPEC-025: the files are mentioned in the session's first message, which Claude
  Code attaches natively; the request file still lists them.)
- **Armed merges.** An armed merge lives on the task's runtime. A restart forgets it; the agent's
  `yulius wait` then still returns on the green checks, and the agent merges.
- **The `yulius` command.** Agents get it as a shim next to the `gh` shim that only allows `wait`.
- **Notices.** "Your pull request is open" and "checks are green" no longer mention promotion.

## Acceptance criteria

1. With the standard profile, after the merge `next_actions` is `Promote` for people, and an agent
   is told not to promote unless asked.
2. A goal task with an attached image gets a request file listing it and a `/goal` naming the file.
3. `delivery_merge` on pending checks arms; on green Yulius merges and the wait returns.
4. Every launch has `--model` (default `opus`) and `autoCompactEnabled: true`.

## Test plan

Domain units (request, briefing), harness launch snapshots, `tests/delivery.rs` (armed merge and
wait endpoint), `tests/task_done.rs` (request file and goal with the fake agent), `wait_cli` units.
Not automated: a real Claude Code session taking the `/goal` and reading an image; checked after
deployment on the instance.

## Plan

- [x] Standard profile has no manual dev checkpoint and legacy profiles read as it
- [x] Request file, definition of done and goal typed after SessionStart with confirmation
- [x] Armed merges and `yulius wait`; briefing and notices end at dev
- [x] Opus by default, auto-compact on, Bash timeouts; docs and changes log; every gate green

## Decisions

- Remove the dev checkpoint instead of auto-confirming it: a confirmation nobody can refuse
  verifies nothing.
- Keep Claude Code's `/goal` (its evaluator keeps the agent going for non-code work too) but give
  it a condition Yulius writes, instead of replacing it with Yulius's own Stop hook.
- Waiting is a background command so Claude Code defers the goal check while it runs and wakes the
  agent when it ends; an MCP call that blocked would keep the turn busy instead.
- Arm on the reviewed head only: a newer head needs a new review.

## Evidence

- `scripts/dev ci` on macOS: see the pull request.
- `cargo test -p yulius --test delivery`: 16 passed, including `an_armed_merge_happens_when_the_checks_pass`.
- `cargo test -p yulius --test task_done`: passed, including `a_goal_task_starts_from_its_request_file`.
- `cargo test -p yulius-domain --lib`: 221 passed; `cargo test -p yulius-harness`: passed.
