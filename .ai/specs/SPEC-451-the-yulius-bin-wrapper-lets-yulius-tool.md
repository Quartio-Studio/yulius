---
yulius-task: 7bccc121-07d2-4f42-852f-27fe6f1ce5f4
status: in-progress
---

# SPEC-451: The /yulius/bin wrapper lets `yulius tool` and `yulius remove` through

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #433, issue #1109
Scope authorization: task #430 (works through open issues), issue #1109 opened by task #418.

## Goal

Every `yulius` command made for a task's session works there: `yulius tool <name> [<json>]`
(SPEC-432 MCR-005) and `yulius remove <paths>` (SPEC-192 RMV-002), which the briefing tells agents
to use, run instead of printing the wrapper's usage line.

## Context

- **Symptom.** In a task session `yulius tool steps_get '{}'` prints "In a task's session yulius
  offers: yulius wait [--minutes N], yulius security-scan [--all]" and exits 2, while
  `/usr/local/bin/yulius tool steps_get '{}'` prints the plan. Reproduced in task #433's own
  session on 2026-11-11; `yulius remove --help` gives the same usage line and exit 2.
- **Code path.** `PATH` of an agent run starts with `/yulius/bin` (`put_gh_first`,
  `crates/yulius-app/src/identity.rs`), which holds the `gh` shim and the `yulius` wrapper written
  from `agent_yulius_shim`. The wrapper is an allowlist (`case "$1" in wait|security-scan) exec …`)
  so that a session cannot reach the server's other subcommands (`serve`, `sandbox-base`, …).
- **Root cause.** The allowlist is a second list of the session's commands, kept by hand. It
  dates from SPEC-011 (`wait` only); SPEC-195 added `security-scan`, but `remove` (SPEC-192,
  #588, 2026-10-16) and `tool` (SPEC-432, #1099, 2026-11-07) were added to the CLI and the
  briefing without it. Nothing ties the two lists together.
- **Why the tests missed it.** `the_agent_yulius_shim_offers_wait_and_the_security_scan` only
  checks the commands the wrapper already had; `remove_cli` and `tool_cli` tests call the binary
  directly, never through the wrapper.
- **Existing sessions.** The server rewrites the wrapper when its text changes (`write_gh_shim`,
  atomically by rename) whenever it prepares an agent run or a user's space, into the host
  directory that every run binds as a directory at `/yulius/bin`. So after the update the first
  run or shell any task starts rewrites it, and sessions already running see the new file too
  (a directory bind shows a renamed-in entry); nothing needs a restart.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| WRP-001 | The agent's `yulius` wrapper passes `wait`, `tool`, `security-scan` and `remove` with their arguments to the real binary and keeps its exit code; anything else prints a usage line naming `yulius wait [--minutes N]`, `yulius tool <name> [<json>]`, `yulius security-scan [--all]` and `yulius remove <paths>` and exits 2. | `identity::tests::the_agent_yulius_shim_offers_the_session_commands` |
| WRP-002 | Every CLI subcommand documented as "In a task's session" is one the wrapper passes, so a new session command cannot be left out again. | `tests::the_agent_wrapper_passes_every_session_command` (`crates/yulius/src/main.rs`) |

## Plan

- [x] Spec of the wrapper fix
- [ ] Regression test: the wrapper passes tool and remove
- [ ] The wrapper passes tool and remove and its usage names them
- [ ] Docs and changes entry
- [ ] Independent review answered and gates green

## Acceptance criteria

- Through the generated wrapper, `tool steps_get '{}'` and `remove a b` reach the binary with the
  same arguments and exit code; `serve` is refused with exit 2 and a usage line naming
  `yulius tool <name> [<json>]`.
- A test fails when a subcommand whose help starts "In a task's session" is not passed by the
  wrapper.
- `yulius tool steps_get '{}'` in a real task session prints the plan (only checkable once the
  change is deployed; see Evidence).

## Test plan

- Regression (unit, `identity.rs`): the existing wrapper test, renamed, also runs `tool steps_get
  '{}'` and `remove a b` through the generated wrapper against a fake binary and checks the usage
  line names `yulius tool <name> [<json>]` (fails today: exit 2 with the old usage line).
- Guard (unit, `main.rs`): reads clap's subcommands, takes those whose about starts with "In a
  task's session" and checks each is in the wrapper's `case` (fails today on `tool` and `remove`).
- What else the cause touches: the wrapper still refuses every other subcommand; `tool` and
  `remove` were already meant for sessions and authenticate with the session's task token
  (`YULIUS_TASK_TOKEN`) or refuse paths outside the task (SPEC-192 RMV-005), so letting them
  through grants nothing new. The briefing is unchanged.
- Gates: `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **`remove` is fixed here too.** It fails the same way for the same cause; leaving it would ship
  a fix that still breaks a command the briefing names. Same file, same line.
- **Keep the allowlist; guard it with a test.** Passing every subcommand would hand sessions
  `serve`, `sandbox-base` and the internals; generating the `case` from clap would need the
  binary's CLI types in `yulius-app`, the wrong direction. A test in the binary that compares the
  two lists catches the drift at the gate, which is what failed twice.
